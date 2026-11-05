// SPDX-License-Identifier: MPL-2.0
//! Why a station cannot be played, in words the listener can act on.
//!
//! A failed connection does not say by itself whose fault it is: a server that does not answer
//! and an Internet access that is down look the same. After a failure the decoder asks
//! [`internet_reachable`]; if Internet works, the station is at fault and [`Failure::reason`]
//! says how (server stopped, stream removed, not audio…); otherwise the connection is.

use std::error::Error as _;
use std::io;
use std::time::Duration;

use stream_download::http::reqwest::{self, Client};
use stream_download::http::HttpStreamError;

/// Servers asked whether Internet works: the Radio-Browser API, which Soundfolk already uses for
/// its catalog (no new third party learns anything). Any HTTPS answer counts, even an error: it
/// proves a real connection (a captive portal cannot fake the certificate).
const INTERNET_CHECKS: [&str; 2] = ["https://all.api.radio-browser.info/", "https://de1.api.radio-browser.info/"];
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

impl Failure {
    /// What the station does wrong, to follow "The station is not broadcasting: ".
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::UnknownHost => "its server's address no longer exists".into(),
            Self::Refused => "its server refuses connections".into(),
            Self::NoAnswer => "its server does not answer".into(),
            Self::Unreachable(detail) => format!("its server cannot be reached ({detail})"),
            Self::Http(status @ (404 | 410)) => format!("the stream no longer exists (HTTP {status})"),
            Self::Http(status @ 500..) => format!("its server has a problem (HTTP {status})"),
            Self::Http(status) => format!("its server refuses the stream (HTTP {status})"),
            Self::Stopped(_) => "the stream stopped".into(),
            Self::NotAudio(detail) => format!("it does not send playable audio ({detail})"),
            Self::Internal(detail) => detail.clone(),
        }
    }

    /// The connection to the station's server failed.
    pub(crate) fn connect(error: &HttpStreamError<Client>) -> Self {
        match error {
            HttpStreamError::ResponseFailure(failure) => Self::Http(failure.response().status().as_u16()),
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

/// Whether Internet works, apart from the station. Blocking, at most a few seconds.
pub(crate) fn internet_reachable(runtime: &tokio::runtime::Handle) -> bool {
    let Ok(client) = Client::builder()
        .user_agent(concat!("Soundfolk/", env!("CARGO_PKG_VERSION")))
        .timeout(INTERNET_CHECK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return true; // cannot tell: blame nobody, the station's own error is shown
    };
    runtime.block_on(async {
        let (a, b) = tokio::join!(client.head(INTERNET_CHECKS[0]).send(), client.head(INTERNET_CHECKS[1]).send());
        a.is_ok() || b.is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use stream_download::http::HttpStream;

    async fn connect(url: &str) -> Failure {
        let client = Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
        match HttpStream::new(client, url.parse().unwrap()).await {
            Ok(_) => panic!("{url} should fail"),
            Err(e) => Failure::connect(&e),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_server_refuses_connections() {
        // A port that was just freed: nothing listens there.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(connect(&format!("http://127.0.0.1:{port}/live")).await, Failure::Refused);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_removed_stream_is_an_http_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/gone.mp3", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let _ = client.read(&mut [0u8; 1024]);
            let _ = client.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        });
        let failure = connect(&url).await;
        assert_eq!(failure, Failure::Http(404));
        assert_eq!(failure.reason(), "the stream no longer exists (HTTP 404)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_name_is_an_unknown_host() {
        // `.invalid` never resolves (RFC 2606), with or without network.
        assert_eq!(connect("http://soundfolk-test.invalid/live").await, Failure::UnknownHost);
    }

    #[test]
    fn reasons_read_after_the_lead() {
        assert_eq!(Failure::Refused.reason(), "its server refuses connections");
        assert_eq!(Failure::Http(503).reason(), "its server has a problem (HTTP 503)");
        assert_eq!(Failure::Http(403).reason(), "its server refuses the stream (HTTP 403)");
        assert_eq!(Failure::Stopped("ended".into()).reason(), "the stream stopped");
    }
}
