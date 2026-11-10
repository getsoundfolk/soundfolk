// SPDX-License-Identifier: MPL-2.0
//! Why a station cannot be played, in words the listener can act on — and never a wrong culprit.
//!
//! A failed connection does not say by itself whose fault it is: a server that does not answer
//! and an Internet access that is down look the same. [`diagnose`] only blames someone on proof:
//! * **the station**, when its address answered (an HTTP client error), or when Internet is shown
//!   to work (a Radio-Browser server answers);
//! * **the Internet access**, when two independent signals agree: Soundfolk's own check fails
//!   *and* the operating system says there is no Internet (see [`super::connectivity`]);
//! * otherwise **nobody**: the player says what it sees ("the server does not answer") without
//!   guessing why.

use std::error::Error as _;
use std::future::Future;
use std::io;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{self, Client};
use stream_download::http::HttpStreamError;

use tokio::sync::watch;

use super::connectivity::Connectivity;
use super::Cancel;
use std::sync::Arc;

/// Servers asked whether Internet works: the Radio-Browser API, which Soundfolk already uses for
/// its catalog (no new third party learns anything). Any HTTPS answer counts, even an error: it
/// proves a real connection (a captive portal cannot fake the certificate).
const INTERNET_CHECKS: [&str; 2] = [
    "https://all.api.radio-browser.info/",
    "https://de1.api.radio-browser.info/",
];
const INTERNET_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a playback attempt failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The station's server name does not resolve.
    UnknownHost,
    /// The station's server refuses connections (nothing listens: server stopped).
    Refused,
    /// The station's server does not answer in time.
    NoAnswer,
    /// The station's server cannot be reached for another reason (TLS, reset…).
    Unreachable(String),
    /// The station's server answers with an HTTP error.
    Http(u16),
    /// The stream ended or stopped sending.
    Stopped(String),
    /// What the server sends cannot be played.
    NotAudio(String),
    /// On Soundfolk's side (sound conversion, HTTP client): retrying does not help.
    Internal(String),
}

/// Whose fault a failure is, as far as can be proven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Internet works: the station is at fault.
    Station,
    /// Proven: no Internet access.
    NoInternet,
    /// Proven: a captive portal wants the user to sign in.
    SignIn,
    /// Cannot be told apart: no culprit is named.
    Unknown,
}

/// Whose fault `failure` is, on proof only (see the module). Blocking, at most a few seconds,
/// but returns at once when `cancel` fires (the verdict is then meaningless: the caller checks
/// the cancellation first). The Internet check is dropped; the system's, blocking, ends on its own
/// timeout in the background without holding the caller.
pub(crate) fn diagnose(
    failure: &Failure,
    runtime: &tokio::runtime::Handle,
    cancel: &Arc<Cancel>,
) -> Verdict {
    let (stop, stopped) = watch::channel(false);
    let _hook = cancel.on_cancel_scoped(move || {
        let _ = stop.send(true);
    });
    decide(
        failure,
        || unless_cancelled(runtime, stopped.clone(), internet_reachable()).unwrap_or(false),
        || {
            // D-Bus calls, blocking: on the blocking pool so that a cancellation is not held up.
            let system = runtime.spawn_blocking(super::connectivity::system);
            unless_cancelled(runtime, stopped.clone(), system)
                .and_then(Result::ok)
                .flatten()
        },
    )
}

/// Runs `task` to completion, or `None` as soon as `stopped` turns `true`.
fn unless_cancelled<T>(
    runtime: &tokio::runtime::Handle,
    mut stopped: watch::Receiver<bool>,
    task: impl Future<Output = T>,
) -> Option<T> {
    runtime.block_on(async {
        tokio::select! {
            out = task => Some(out),
            // If the sender is dropped without a stop, this branch is disabled and `task` runs
            // to its end.
            Ok(_) = stopped.wait_for(|stopped| *stopped) => None,
        }
    })
}

/// [`diagnose`], with the checks passed in. They only run when the failure itself proves nothing.
fn decide(
    failure: &Failure,
    internet_reachable: impl FnOnce() -> bool,
    system: impl FnOnce() -> Option<Connectivity>,
) -> Verdict {
    match failure {
        // "Network Authentication Required" (RFC 6585): sent by captive portals, never by a station.
        Failure::Http(511) => return Verdict::SignIn,
        // An HTTP client error came from the station's address (a 5xx can come from a proxy
        // that cannot reach it, and goes through the checks).
        Failure::Http(400..=499) => return Verdict::Station,
        _ => {}
    }
    if internet_reachable() {
        return Verdict::Station;
    }
    // Soundfolk's check failed: it is no proof alone (Radio-Browser may be down or blocked).
    match system() {
        Some(Connectivity::NoInternet) => Verdict::NoInternet,
        Some(Connectivity::Portal) => Verdict::SignIn,
        Some(Connectivity::Full) | None => Verdict::Unknown,
    }
}

impl Failure {
    /// What the station does wrong, once proven, to follow "The station is not broadcasting: ".
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::UnknownHost => "its server's address no longer exists".into(),
            Self::Refused => "its server refuses connections".into(),
            Self::NoAnswer => "its server does not answer".into(),
            Self::Unreachable(detail) => format!("its server cannot be reached ({detail})"),
            // On Icecast and Shoutcast, a 404 is also a mount whose source is not connected right
            // now (off air, broadcast hours): only a 410 says the stream is gone for good.
            Self::Http(404) => "the stream is not on air (HTTP 404)".into(),
            Self::Http(410) => "the stream no longer exists (HTTP 410)".into(),
            Self::Http(status @ 500..) => format!("its server has a problem (HTTP {status})"),
            Self::Http(status) => format!("the stream is refused (HTTP {status})"),
            Self::Stopped(_) => "the stream stopped".into(),
            Self::NotAudio(detail) => format!("it does not send playable audio ({detail})"),
            Self::Internal(detail) => detail.clone(),
        }
    }

    /// What was seen, when nobody can be blamed: facts only, no cause ("cannot be found", not
    /// "no longer exists"; "the connection was cut", not "the stream stopped").
    pub(crate) fn symptom(&self) -> String {
        match self {
            Self::UnknownHost => "its server's address cannot be found".into(),
            Self::Stopped(_) => "the connection was cut".into(),
            Self::NotAudio(_) => "what it sends is not audio".into(),
            other => other.reason(),
        }
    }

    /// The connection to the station's server failed.
    pub(crate) fn connect(error: &HttpStreamError<Client>) -> Self {
        match error {
            HttpStreamError::ResponseFailure(failure) => {
                Self::Http(failure.response().status().as_u16())
            }
            HttpStreamError::FetchFailure(error) => Self::request(error),
        }
    }

    fn request(error: &reqwest::Error) -> Self {
        if let Some(status) = error.status() {
            return Self::Http(status.as_u16());
        }
        let mut source = error.source();
        let mut deepest = error.to_string();
        while let Some(cause) = source {
            if let Some(io) = cause.downcast_ref::<io::Error>() {
                match io.kind() {
                    io::ErrorKind::ConnectionRefused => return Self::Refused,
                    io::ErrorKind::TimedOut => return Self::NoAnswer,
                    _ => {}
                }
            }
            // hyper's resolver error is private: only its message says so.
            if cause.to_string().starts_with("dns error") {
                return Self::UnknownHost;
            }
            deepest = cause.to_string();
            source = cause.source();
        }
        if error.is_timeout() {
            Self::NoAnswer
        } else {
            Self::Unreachable(deepest)
        }
    }
}

/// Whether Internet works, apart from the station: at most a few seconds.
async fn internet_reachable() -> bool {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    let Ok(client) = crate::http::cached(&CLIENT, || {
        crate::http::builder()
            .timeout(INTERNET_CHECK_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
    }) else {
        return true; // cannot tell: blame nobody, the station's own error is shown
    };
    let (a, b) = tokio::join!(
        client.head(INTERNET_CHECKS[0]).send(),
        client.head(INTERNET_CHECKS[1]).send()
    );
    a.is_ok() || b.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use stream_download::http::HttpStream;

    async fn connect(url: &str) -> Failure {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        match HttpStream::new(client, url.parse().unwrap()).await {
            Ok(_) => panic!("{url} should fail"),
            Err(e) => Failure::connect(&e),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_server_refuses_connections() {
        // A port that was just freed: nothing listens there.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert_eq!(
            connect(&format!("http://127.0.0.1:{port}/live")).await,
            Failure::Refused
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_missing_stream_is_an_http_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/gone.mp3", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let _ = client.read(&mut [0u8; 1024]);
            let _ = client.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        });
        let failure = connect(&url).await;
        assert_eq!(failure, Failure::Http(404));
        // Not "no longer exists": an Icecast mount answers 404 while its source is off air
        assert_eq!(failure.reason(), "the stream is not on air (HTTP 404)");
        assert_eq!(
            Failure::Http(410).reason(),
            "the stream no longer exists (HTTP 410)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_name_is_an_unknown_host() {
        // `.invalid` never resolves (RFC 2606), with or without network.
        assert_eq!(
            connect("http://soundfolk-test.invalid/live").await,
            Failure::UnknownHost
        );
    }

    #[test]
    fn a_stop_cuts_a_check_short() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (stop, stopped) = watch::channel(false);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let _ = stop.send(true);
        });
        // A check that never answers: only the stop ends it.
        let check = std::future::pending::<bool>();
        assert_eq!(unless_cancelled(runtime.handle(), stopped, check), None);
    }

    #[test]
    fn a_check_dropped_without_a_stop_runs_to_its_end() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (stop, stopped) = watch::channel(false);
        drop(stop);
        let check = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            true
        };
        assert_eq!(
            unless_cancelled(runtime.handle(), stopped, check),
            Some(true)
        );
    }

    #[test]
    fn a_stopped_playback_is_not_diagnosed() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let cancel = Arc::new(Cancel::default());
        cancel.cancel();
        let start = std::time::Instant::now();
        assert_eq!(
            diagnose(&Failure::NoAnswer, runtime.handle(), &cancel),
            Verdict::Unknown
        );
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn reasons_read_after_the_lead() {
        assert_eq!(Failure::Refused.reason(), "its server refuses connections");
        assert_eq!(
            Failure::Http(503).reason(),
            "its server has a problem (HTTP 503)"
        );
        assert_eq!(
            Failure::Http(403).reason(),
            "the stream is refused (HTTP 403)"
        );
        assert_eq!(
            Failure::Stopped("ended".into()).reason(),
            "the stream stopped"
        );
    }

    #[test]
    fn symptoms_name_no_cause() {
        assert_eq!(
            Failure::UnknownHost.symptom(),
            "its server's address cannot be found"
        );
        assert_eq!(
            Failure::Stopped("reset".into()).symptom(),
            "the connection was cut"
        );
        assert_eq!(Failure::NoAnswer.symptom(), "its server does not answer");
    }

    #[test]
    fn answers_from_the_station_or_a_portal_need_no_check() {
        let unused = || -> bool { panic!("no check needed") };
        let no_system = || -> Option<Connectivity> { panic!("no check needed") };
        assert_eq!(
            decide(&Failure::Http(404), unused, no_system),
            Verdict::Station
        );
        assert_eq!(
            decide(&Failure::Http(511), unused, no_system),
            Verdict::SignIn
        );
    }

    #[test]
    fn internet_working_blames_the_station() {
        let never =
            || -> Option<Connectivity> { panic!("not asked once Internet is shown to work") };
        assert_eq!(decide(&Failure::NoAnswer, || true, never), Verdict::Station);
        // A 5xx may come from a proxy: checked like the rest.
        assert_eq!(
            decide(&Failure::Http(502), || true, never),
            Verdict::Station
        );
    }

    #[test]
    fn no_internet_needs_both_signals() {
        let failure = Failure::UnknownHost;
        assert_eq!(
            decide(&failure, || false, || Some(Connectivity::NoInternet)),
            Verdict::NoInternet
        );
        assert_eq!(
            decide(&failure, || false, || Some(Connectivity::Portal)),
            Verdict::SignIn
        );
        // Only Soundfolk's check failed (Radio-Browser down or blocked): nobody is blamed.
        assert_eq!(
            decide(&failure, || false, || Some(Connectivity::Full)),
            Verdict::Unknown
        );
        assert_eq!(decide(&failure, || false, || None), Verdict::Unknown);
        assert_eq!(
            decide(&Failure::Http(503), || false, || None),
            Verdict::Unknown
        );
    }
}
