// SPDX-License-Identifier: MPL-2.0
//! Network → decoding pipeline: runs on a dedicated thread (`audio-decoder`).
//!
//! 1. `stream-download` fetches the HTTP stream into a **bounded** in-memory buffer (a live stream is endless).
//! 2. `symphonia` detects the format (MP3, AAC, Ogg/Vorbis, FLAC...) and decodes to PCM.
//! 3. [`Converter`] adapts channels and sample rate to the output device.
//! 4. Samples are pushed into the ring buffer read by the cpal callback.
//!
//! If the connection drops *after* the first sound, the stream is reopened automatically, for
//! as long as it takes when the network is down (sleep, Wi-Fi lost): the attempt starts as soon as
//! it is back (see [`super::network`]). A connection still open across a network change or a
//! sleep is dropped as soon as it stops receiving, instead of waiting for the read timeout.
//! After a failure, the user is told whose fault it is, **only when proven** (see
//! [`super::failure`]): no network, no Internet access, a captive portal (waited for as long as it
//! takes), or the station itself (its server stopped, the stream was removed…: retried, then given
//! up; on the very first connection, reported at once). Without proof, the player says what it
//! sees and keeps trying.

use std::io;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rtrb::Producer;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use stream_download::http::reqwest::header::{HeaderMap, HeaderValue};
use stream_download::http::reqwest::{Client, Url};
use stream_download::http::HttpStream;
use stream_download::storage::bounded::BoundedStorageProvider;
use stream_download::storage::memory::MemoryStorageProvider;
use stream_download::{Settings, StreamDownload};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTag};
use tokio::sync::watch;

use super::failure::{self, Failure};
use super::icy::{IcyReader, TitleCallback};
use super::loudness::StationLevel;
use super::network::NetState;
use super::output::OutputFormat;
use super::{Cancel, Reporter};

/// Bytes to receive before starting to read. Just enough to detect the format: Icecast
/// servers send ~30 KB in a burst on connect, then only at playback pace,
/// so requiring more (e.g. 64 KB) delays the first sound accordingly, with no robustness gain.
/// Dropout protection comes from the bounded buffer below, which fills whenever the network
/// delivers faster than playback (catching up after a slowdown).
const PREFETCH_BYTES: u64 = 16 * 1024;
/// Size of the stream's circular buffer (~1 min at 128 kbit/s).
const BUFFER_BYTES: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A live stream that delivers nothing for this long is considered dropped (it delivers several
/// times per second; the buffers cover this gap). Catches what the network watch cannot see,
/// like a gateway that dies with the link still up.
const READ_TIMEOUT: Duration = Duration::from_secs(8);
/// After a network change or a wake from sleep, a connection that receives nothing for this long
/// is dead (a live stream delivers several times per second). One that keeps receiving is kept:
/// switching Ethernet → Wi-Fi with both up does not cut the sound.
const STALL_AFTER_CHANGE: Duration = Duration::from_secs(3);
/// Longest wait between two attempts while the network is up.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// The station is given up after failing this long **with the network up** (time offline or
/// asleep does not count).
const GIVE_UP_AFTER: Duration = Duration::from_secs(5 * 60);

const RECONNECTING: &str = "Reconnecting…";
const NO_NETWORK: &str = "No network — waiting…";
const NO_INTERNET: &str = "No Internet access — waiting…";
const SIGN_IN: &str = "Sign in to the network (captive portal) — waiting…";

pub struct Job {
    pub url: Url,
    pub format: OutputFormat,
    pub producer: Producer<f32>,
    /// *Constant volume*: measures the station and applies its gain. Kept across reconnections
    /// (same station); its level is saved when the playback ends.
    pub level: StationLevel,
    pub reporter: Reporter,
    pub cancel: Arc<Cancel>,
    pub runtime: tokio::runtime::Handle,
    pub net: watch::Receiver<NetState>,
}

pub fn spawn(job: Job) -> io::Result<()> {
    thread::Builder::new()
        .name("audio-decoder".into())
        .spawn(move || run(job))?;
    Ok(())
}

/// Why a playback attempt ended.
enum End {
    Cancelled,
    /// Stream dropped or unreadable.
    Failed(Failure),
    /// Dropped on purpose: the network changed or the computer slept, and data stopped coming.
    Interrupted(Interrupt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interrupt {
    Network,
    Wake,
}

impl std::fmt::Display for Interrupt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Network => "the network changed",
            Self::Wake => "the computer woke from sleep",
        })
    }
}

fn run(mut job: Job) {
    let client = match build_client() {
        Ok(client) => client,
        Err(message) => return job.reporter.failed(message),
    };

    let mut ever_played = false;
    let mut attempts = 0;
    // Since when the station itself fails (time without network or Internet does not count).
    let mut failing_since: Option<Instant> = None;

    loop {
        let mut played = false;
        let end = play_once(&mut job, &client, &mut played);
        ever_played |= played;
        if played {
            attempts = 0;
            failing_since = None;
        }

        let failure = match end {
            End::Cancelled => return,
            End::Failed(failure) => Some(failure),
            End::Interrupted(why) => {
                tracing::info!(url = %job.url, "connection dropped: {why}");
                None
            }
        };
        if job.cancel.is_cancelled() {
            return;
        }
        let interrupted = failure.is_none();
        let message = match failure {
            None => RECONNECTING.to_owned(),
            // Retrying would fail the same way.
            Some(Failure::Internal(message)) => return job.reporter.failed(message),
            Some(failure) => {
                tracing::warn!(url = %job.url, ?failure, "playback failed");
                // Right after the sound stopped, the check is shown; later the last reason stays.
                match blame(&job, ever_played && attempts == 0, &failure) {
                    Blame::Connection(message) => {
                        failing_since = None;
                        message.to_owned()
                    }
                    // Nobody can be blamed: what was seen, and no giving up (it may be the
                    // connection, which is waited for as long as it takes).
                    Blame::Unknown => {
                        failing_since = None;
                        format!(
                            "Cannot play the station ({}) — retrying…",
                            failure.symptom()
                        )
                    }
                    Blame::Station => {
                        let reason = failure.reason();
                        // Never played: dead URL, not audio… the user must be told right away.
                        if !ever_played {
                            return job
                                .reporter
                                .failed(format!("The station is not broadcasting: {reason}"));
                        }
                        if failing_since.get_or_insert_with(Instant::now).elapsed() > GIVE_UP_AFTER
                        {
                            return job
                                .reporter
                                .failed(format!("The station stopped broadcasting: {reason}"));
                        }
                        format!("The station is not broadcasting ({reason}) — retrying…")
                    }
                }
            }
        };

        attempts += 1;
        // A connection dropped on purpose is reopened at once; then exponential backoff
        // (1 s, 2 s, 4 s… capped), cut short when the network comes back or changes.
        let delay = match (interrupted, attempts) {
            (true, 1) => Duration::ZERO,
            _ => Duration::from_secs(1u64 << (attempts - 1).min(5)).min(MAX_BACKOFF),
        };
        if !wait_before_retry(&job, delay, &message) {
            return;
        }
    }
}

/// Whose fault a failure is, on proof only (see [`failure::diagnose`]).
enum Blame {
    /// No network, no Internet access, or a captive portal: waited for as long as it takes.
    Connection(&'static str),
    /// Internet works: the station is at fault.
    Station,
    /// Cannot be told: nobody is named.
    Unknown,
}

fn blame(job: &Job, sound_just_stopped: bool, failure: &Failure) -> Blame {
    if !job.net.borrow().online {
        return Blame::Connection(NO_NETWORK);
    }
    if sound_just_stopped {
        // The check takes a moment: the player must not look like it still plays.
        job.reporter.reconnecting(RECONNECTING);
    }
    match failure::diagnose(failure, &job.runtime, &job.cancel) {
        failure::Verdict::Station => Blame::Station,
        failure::Verdict::NoInternet => Blame::Connection(NO_INTERNET),
        failure::Verdict::SignIn => Blame::Connection(SIGN_IN),
        failure::Verdict::Unknown => Blame::Unknown,
    }
}

/// Waits `delay` showing `message`, or for the network if it is down; returns at once when the
/// network comes back or changes. `false` if the playback was stopped meanwhile.
fn wait_before_retry(job: &Job, delay: Duration, message: &str) -> bool {
    let mut net = job.net.clone();
    net.mark_unchanged();
    let deadline = tokio::time::Instant::now() + delay;
    // Without network news (the watch thread is gone), only the delay and the stop are waited for.
    let mut watched = true;
    loop {
        if job.cancel.is_cancelled() {
            return false;
        }
        let state = *net.borrow_and_update();
        job.reporter
            .reconnecting(if state.online { message } else { NO_NETWORK });
        if state.online && tokio::time::Instant::now() >= deadline {
            return true;
        }
        // Offline, the delay does not count: only the network coming back ends the wait.
        let changed = job.runtime.block_on(async {
            tokio::select! {
                () = job.cancel.cancelled() => false,
                news = net.changed(), if watched => {
                    watched = news.is_ok();
                    watched
                }
                () = tokio::time::sleep_until(deadline), if state.online => false,
            }
        });
        if changed && net.borrow().online {
            tracing::info!("network back or changed: reconnecting now");
            return !job.cancel.is_cancelled();
        }
    }
}

/// What a stream probe learned.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub station: Option<String>,
    pub mime: Option<String>,
    pub sample_rate: u32,
    pub channels: usize,
}

/// Checks that a stream connects, is detected **and decodes** (first audio packet), without playing
/// anything. Blocking: call from a dedicated thread (`spawn_blocking`), never from an async task.
pub fn probe(url: &Url, runtime: &tokio::runtime::Handle) -> Result<StreamInfo, String> {
    // Packets skipped before giving up (truncated stream start, ID3 headers...).
    const MAX_PACKETS: usize = 64;

    let client = build_client()?;
    let cancel = Cancel::default();
    let mut opened = match open_stream(
        &client,
        url,
        runtime,
        &cancel,
        Arc::new(|_| {}),
        std::time::Instant::now(),
        None,
    ) {
        Ok(opened) => opened,
        Err(End::Failed(failure)) => {
            return Err(format!("Cannot play this stream: {}", failure.reason()))
        }
        Err(End::Interrupted(why)) => return Err(format!("Test interrupted: {why}")),
        Err(End::Cancelled) => return Err("Test cancelled".into()),
    };

    for _ in 0..MAX_PACKETS {
        let packet = match opened.format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => return Err("The stream ended before sending any audio".into()),
            Err(e) => return Err(format!("Cannot read stream: {e}")),
        };
        if packet.track_id != opened.track_id {
            continue;
        }
        if let Ok(decoded) = opened.decoder.decode(&packet) {
            if decoded.frames() > 0 {
                let spec = decoded.spec();
                // Closing the stream (drop) stops the download.
                cancel.cancel();
                return Ok(StreamInfo {
                    station: opened.station,
                    mime: opened.mime,
                    sample_rate: spec.rate(),
                    channels: spec.channels().count(),
                });
            }
        }
    }
    cancel.cancel();
    Err("No decodable audio in the stream".into())
}

/// Stream that is open, detected and ready to decode.
struct OpenedStream {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    mime: Option<String>,
    /// Station name announced by the server (`icy-name`).
    station: Option<String>,
    /// HTTP `Server` header (Icecast, Shoutcast…).
    server: Option<String>,
    /// The stream can announce titles itself (ICY, or tags in an Ogg container).
    inband: bool,
    /// Drops the connection if it stalls after a network change (stopped with the stream).
    _watchdog: Option<Watchdog>,
}

/// Why the watchdog dropped the connection, read once the decoder sees the stream end.
type Fired = Arc<Mutex<Option<Interrupt>>>;

/// What the watchdog needs: the network state, and where to say why it fired.
struct NetGuard {
    net: watch::Receiver<NetState>,
    fired: Fired,
}

/// When the connection last received data.
struct Activity {
    base: Instant,
    last_ms: AtomicU64,
}

impl Activity {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        self.last_ms
            .store(self.base.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle(&self) -> Duration {
        self.base
            .elapsed()
            .saturating_sub(Duration::from_millis(self.last_ms.load(Ordering::Relaxed)))
    }
}

/// Task that drops the connection when it stops receiving after a network change or a sleep.
/// Ends with the stream (`Drop`).
struct Watchdog(tokio::task::JoinHandle<()>);

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Watchdog {
    fn spawn(
        runtime: &tokio::runtime::Handle,
        guard: &NetGuard,
        opened_at: NetState,
        activity: Arc<Activity>,
        stop: impl FnOnce() + Send + 'static,
    ) -> Self {
        let net = guard.net.clone();
        let fired = guard.fired.clone();
        Self(runtime.spawn(async move {
            let mut net = net;
            // Nothing to check before a change; returns at once if one came since the opening.
            let changed =
                |now: &NetState| now.wake != opened_at.wake || now.network != opened_at.network;
            if net.wait_for(changed).await.is_err() {
                return; // the network is no longer watched
            }
            // Then the stream has until STALL_AFTER_CHANGE without data: wait exactly that long
            // after the last data received, again and again while data keeps coming.
            loop {
                let idle = activity.idle();
                if idle < STALL_AFTER_CHANGE {
                    tokio::time::sleep(STALL_AFTER_CHANGE - idle).await;
                    continue;
                }
                let now = *net.borrow();
                let why = if now.wake != opened_at.wake {
                    Interrupt::Wake
                } else {
                    Interrupt::Network
                };
                tracing::info!("{why}, and the stream stopped receiving: reconnecting");
                *fired.lock().unwrap_or_else(|p| p.into_inner()) = Some(why);
                stop();
                return;
            }
        }))
    }
}

fn build_client() -> Result<Client, String> {
    // Request ICY metadata; a server that does not support it simply ignores the header.
    let mut headers = HeaderMap::new();
    headers.insert("Icy-MetaData", HeaderValue::from_static("1"));

    Client::builder()
        .default_headers(headers)
        .user_agent(concat!("Soundfolk/", env!("CARGO_PKG_VERSION")))
        // The stream is read as raw bytes (ICY blocks, range requests): never compressed.
        .no_gzip()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client unavailable: {e}"))
}

/// Connection, network buffer, ICY metadata stripping, format detection, decoder creation.
fn open_stream(
    client: &Client,
    url: &Url,
    runtime: &tokio::runtime::Handle,
    cancel: &Cancel,
    on_title: TitleCallback,
    started: std::time::Instant,
    guard: Option<NetGuard>,
) -> Result<OpenedStream, End> {
    let activity = Arc::new(Activity::new());
    let opened_at = guard.as_ref().map(|g| *g.net.borrow());

    // ── 1. Connection + network buffer ──────────────────────────────────────
    let progress = activity.clone();
    let connect = async {
        let http = HttpStream::new(client.clone(), url.clone())
            .await
            .map_err(|e| Failure::connect(&e))?;
        tracing::debug!(
            ms = started.elapsed().as_millis() as u64,
            "HTTP headers received"
        );
        let mime = http
            .content_type()
            .as_ref()
            .map(|ct| format!("{}/{}", ct.r#type, ct.subtype));
        let metaint = http
            .header("icy-metaint")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0);
        let station = http
            .header("icy-name")
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty());
        let server = http
            .header("server")
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        let storage = BoundedStorageProvider::new(
            MemoryStorageProvider,
            NonZeroUsize::new(BUFFER_BYTES).expect("BUFFER_BYTES > 0"),
        );
        let reader = StreamDownload::from_stream(
            http,
            storage,
            Settings::default()
                .prefetch_bytes(PREFETCH_BYTES)
                .on_progress(move |_, _, _| progress.touch()),
        )
        .await
        .map_err(|e| Failure::Stopped(format!("cannot read the stream: {e}")))?;
        tracing::debug!(
            ms = started.elapsed().as_millis() as u64,
            "prefetch complete"
        );
        Ok::<_, Failure>((reader, mime, metaint, station, server))
    };
    // A connection attempt on a network that just went away can hang until its timeout: a
    // change of network starts a new attempt at once.
    let opened = runtime.block_on(async {
        let Some(guard) = &guard else {
            return connect.await.map_err(End::Failed);
        };
        let mut net = guard.net.clone();
        net.mark_unchanged();
        tokio::select! {
            opened = connect => opened.map_err(End::Failed),
            _ = net.changed() => Err(End::Interrupted(Interrupt::Network)),
        }
    });
    let (reader, mime, metaint, station, server) = opened?;

    // Cancelling = stopping the download, which unblocks a read waiting for data.
    let token = reader.cancellation_token();
    let stop = token.clone();
    cancel.on_cancel(move || token.cancel());
    if cancel.is_cancelled() {
        return Err(End::Cancelled);
    }
    activity.touch();
    let watchdog = guard
        .as_ref()
        .zip(opened_at)
        .map(|(g, at)| Watchdog::spawn(runtime, g, at, activity, move || stop.cancel()));

    // ── 2. Format detection ──────────────────────────────────────────────────
    let mut hint = Hint::new();
    if let Some(mime) = &mime {
        hint.mime_type(mime);
    }
    // A live stream is neither seekable nor of known length: `ReadOnlySource` tells symphonia so.
    // With ICY, metadata blocks are stripped before symphonia reads the stream.
    let source: Box<dyn MediaSource> = match metaint {
        Some(metaint) => Box::new(ReadOnlySource::new(IcyReader::new(
            reader, metaint, on_title,
        ))),
        None => Box::new(ReadOnlySource::new(reader)),
    };
    let source = MediaSourceStream::new(source, Default::default());
    let format = match symphonia::default::get_probe().probe(
        &hint,
        source,
        FormatOptions::default(),
        MetadataOptions::default(),
    ) {
        Ok(format) => format,
        Err(e) if cancel.is_cancelled() => {
            tracing::debug!("detection interrupted: {e}");
            return Err(End::Cancelled);
        }
        // The stream broke while being read: not a format problem.
        Err(SymphoniaError::IoError(e)) => {
            return Err(End::Failed(Failure::Stopped(e.to_string())))
        }
        Err(e) => {
            return Err(End::Failed(Failure::NotAudio(format!(
                "unrecognized format: {e}"
            ))))
        }
    };

    tracing::debug!(ms = started.elapsed().as_millis() as u64, "format detected");
    let Some(track) = format.default_track(TrackType::Audio) else {
        return Err(End::Failed(Failure::NotAudio("no audio track".into())));
    };
    let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) else {
        return Err(End::Failed(Failure::NotAudio(
            "audio parameters missing".into(),
        )));
    };
    let track_id = track.id;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| End::Failed(Failure::NotAudio(format!("unsupported codec: {e}"))))?;

    // Ogg streams change title by starting a new chain with new Vorbis comments.
    let inband = metaint.is_some() || format.format_info().short_name == "ogg";
    Ok(OpenedStream {
        format,
        decoder,
        track_id,
        mime,
        station,
        server,
        inband,
        _watchdog: watchdog,
    })
}

fn play_once(job: &mut Job, client: &Client, played: &mut bool) -> End {
    let fired = Fired::default();
    let end = decode(job, client, played, &fired);
    // The stream ended because the watchdog dropped it: say why, so it is reopened at once.
    let why = fired.lock().unwrap_or_else(|p| p.into_inner()).take();
    match (end, why) {
        (End::Failed(_), Some(why)) => End::Interrupted(why),
        (end, _) => end,
    }
}

fn decode(job: &mut Job, client: &Client, played: &mut bool, fired: &Fired) -> End {
    let started = std::time::Instant::now();

    let on_title: TitleCallback = {
        let reporter = job.reporter.clone();
        Arc::new(move |title| {
            tracing::debug!(%title, "current title");
            reporter.title(title);
        })
    };
    let guard = NetGuard {
        net: job.net.clone(),
        fired: fired.clone(),
    };
    let OpenedStream {
        mut format,
        mut decoder,
        mut track_id,
        mime,
        station,
        server,
        inband,
        _watchdog,
    } = match open_stream(
        client,
        &job.url,
        &job.runtime,
        &job.cancel,
        on_title,
        started,
        Some(guard),
    ) {
        Ok(opened) => opened,
        Err(end) => return end,
    };
    job.reporter.station(station);
    job.reporter.stream_opened(inband, server);
    report_tags(&job.reporter, format.metadata().skip_to_latest());
    tracing::info!(url = %job.url, mime = ?mime, "stream opened");

    // ── 3. Decoding ──────────────────────────────────────────────────────────
    let mut converter: Option<Converter> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut converted: Vec<f32> = Vec::new();

    loop {
        if job.cancel.is_cancelled() {
            return End::Cancelled;
        }

        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => return End::Failed(Failure::Stopped("ended by the server".into())),
            Err(_) if job.cancel.is_cancelled() => return End::Cancelled,
            // New Ogg chain (Icecast does this at every title change): new tags, maybe a new
            // track and codec setup. Continue on the same connection with a fresh decoder.
            Err(SymphoniaError::ResetRequired) => {
                report_tags(&job.reporter, format.metadata().skip_to_latest());
                match reset_decoder(format.as_ref()) {
                    Ok((id, new)) => {
                        track_id = id;
                        decoder = new;
                        continue;
                    }
                    Err(message) => return End::Failed(Failure::NotAudio(message)),
                }
            }
            Err(e) => return End::Failed(Failure::Stopped(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // Corrupt or lost packet: common over the network, skip silently.
            Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => continue,
            Err(e) => return End::Failed(Failure::NotAudio(format!("cannot decode: {e}"))),
        };
        if decoded.frames() == 0 {
            continue;
        }

        let spec = decoded.spec();
        let (rate, channels) = (spec.rate(), spec.channels().count());
        // Created on the first packet, recreated if the stream changes format midway.
        if converter.as_ref().map(|c| (c.src_rate, c.src_channels)) != Some((rate, channels)) {
            match Converter::new(rate, channels, job.format) {
                Ok(new) => {
                    tracing::info!(
                        src_rate = rate,
                        src_channels = channels,
                        dst_rate = job.format.sample_rate,
                        resampled = new.resampler.is_some(),
                        "format conversion"
                    );
                    converter = Some(new);
                }
                Err(message) => return End::Failed(Failure::Internal(message)),
            }
        }
        let converter = converter.as_mut().expect("initialized above");

        interleaved.resize(decoded.samples_interleaved(), 0.0);
        decoded.copy_to_slice_interleaved(&mut interleaved);
        converted.clear();
        converter.process(&interleaved, &mut converted);
        let controls = &job.reporter.controls;
        job.level.process(&mut converted, controls.normalize());
        controls.set_station_gain(job.level.leveler.gain_db() as f32);

        if !push_blocking(&mut job.producer, &converted, &job.cancel) {
            return End::Cancelled;
        }
        if !*played {
            *played = true;
            tracing::info!(ms = started.elapsed().as_millis() as u64, "first sound");
            job.reporter.audible();
        }
    }
}

/// Decoder for the stream's audio track after an Ogg chain change.
fn reset_decoder(format: &dyn FormatReader) -> Result<(u32, Box<dyn AudioDecoder>), String> {
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("No audio track in the stream")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("Audio parameters missing")?;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("Unsupported codec: {e}"))?;
    Ok((track.id, decoder))
}

/// Publishes the title carried by the stream's tags (Vorbis comments), if any.
fn report_tags(reporter: &Reporter, revision: Option<&MetadataRevision>) {
    if let Some((artist, title)) = revision.and_then(title_from_tags) {
        tracing::debug!(?artist, %title, "title from stream tags");
        reporter.stream_tags(artist, title);
    }
}

/// Artist and title from a metadata revision (global tags first, then per track).
fn title_from_tags(revision: &MetadataRevision) -> Option<(Option<String>, String)> {
    let tags = revision.media.tags.iter().chain(
        revision
            .per_track
            .iter()
            .flat_map(|t| t.metadata.tags.iter()),
    );
    let (mut artist, mut title) = (None, None);
    for tag in tags {
        match &tag.std {
            Some(StandardTag::TrackTitle(t)) if title.is_none() => title = clean(t),
            Some(StandardTag::Artist(a)) if artist.is_none() => artist = clean(a),
            _ => {}
        }
    }
    Some((artist, title?))
}

fn clean(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Pushes all samples; waits (in 5 ms steps) when the ring buffer is full, which naturally
/// paces decoding — and therefore the download — to actual playback.
/// Returns `false` if the session was cancelled in the meantime.
fn push_blocking(producer: &mut Producer<f32>, mut samples: &[f32], cancel: &Cancel) -> bool {
    while !samples.is_empty() {
        let (_, rest) = producer.push_partial_slice(samples);
        if rest.len() == samples.len() {
            if cancel.is_cancelled() || producer.is_abandoned() {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        samples = rest;
    }
    true
}

/// Block size of the FFT resampler (~21 ms at 48 kHz): latency / CPU cost trade-off.
const RESAMPLER_CHUNK_FRAMES: usize = 1024;

/// Remaps channels then resamples to the output device's format.
///
/// Resampling (`rubato::Fft`, high quality, fixed ratio) works on fixed-size blocks:
/// decoded packets, whose size varies by codec, are accumulated in `pending`.
/// When the sample rates match, it is bypassed (direct copy).
struct Converter {
    src_rate: u32,
    src_channels: usize,
    dst_channels: usize,
    remapped: Vec<f32>,
    resampler: Option<Fft<f32>>,
    /// Remapped frames not yet fed to the resampler (interleaved).
    pending: Vec<f32>,
    /// Output of one block (interleaved), sized to the resampler's maximum.
    scratch: Vec<f32>,
    /// Initial silent frames introduced by the filter, to be discarded.
    skip_frames: usize,
}

impl Converter {
    fn new(src_rate: u32, src_channels: usize, dst: OutputFormat) -> Result<Self, String> {
        let (resampler, scratch, skip_frames) = if src_rate == dst.sample_rate {
            (None, Vec::new(), 0)
        } else {
            let resampler = Fft::<f32>::new(
                src_rate as usize,
                dst.sample_rate as usize,
                RESAMPLER_CHUNK_FRAMES,
                dst.channels,
                FixedSync::Input,
            )
            .map_err(|e| format!("Cannot resample {src_rate} → {} Hz: {e}", dst.sample_rate))?;
            let scratch = vec![0.0; resampler.output_frames_max() * dst.channels];
            let delay = resampler.output_delay();
            (Some(resampler), scratch, delay)
        };

        Ok(Self {
            src_rate,
            src_channels,
            dst_channels: dst.channels,
            remapped: Vec::new(),
            resampler,
            pending: Vec::new(),
            scratch,
            skip_frames,
        })
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.remap(input);
        let Some(resampler) = self.resampler.as_mut() else {
            out.extend_from_slice(&self.remapped);
            return;
        };

        let ch = self.dst_channels;
        self.pending.extend_from_slice(&self.remapped);
        loop {
            let needed = resampler.input_frames_next();
            if self.pending.len() < needed * ch {
                break;
            }
            let capacity = resampler.output_frames_max();
            let (Ok(frames_in), Ok(mut frames_out)) = (
                InterleavedSlice::new(&self.pending[..needed * ch], ch, needed),
                InterleavedSlice::new_mut(&mut self.scratch[..capacity * ch], ch, capacity),
            ) else {
                unreachable!("sizes computed from the resampler");
            };

            match resampler.process_into_buffer(&frames_in, &mut frames_out, None) {
                Ok((read, written)) => {
                    let skip = self.skip_frames.min(written);
                    self.skip_frames -= skip;
                    out.extend_from_slice(&self.scratch[skip * ch..written * ch]);
                    self.pending.drain(..read * ch);
                }
                Err(e) => {
                    // Should not happen; drop the block rather than loop.
                    tracing::warn!("resampling: {e}");
                    self.pending.clear();
                    break;
                }
            }
        }
    }

    fn remap(&mut self, input: &[f32]) {
        let (src, dst) = (self.src_channels, self.dst_channels);
        self.remapped.clear();
        if src == dst {
            self.remapped.extend_from_slice(input);
            return;
        }
        for frame in input.chunks_exact(src) {
            for c in 0..dst {
                self.remapped.push(match (src, dst) {
                    (1, _) => frame[0],
                    // Downmix to mono: average of front left/right.
                    (_, 1) => (frame[0] + frame[1]) * 0.5,
                    _ => frame.get(c).copied().unwrap_or(0.0),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn format(rate: u32, channels: usize) -> OutputFormat {
        OutputFormat {
            sample_rate: rate,
            channels,
        }
    }

    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|n| (TAU * hz * n as f32 / rate as f32).sin())
            .collect()
    }

    /// Frequency estimated by counting rising zero crossings.
    fn frequency(signal: &[f32], rate: u32) -> f32 {
        let crossings = signal
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        crossings as f32 * rate as f32 / signal.len() as f32
    }

    fn rms(signal: &[f32]) -> f32 {
        (signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32).sqrt()
    }

    #[test]
    fn same_format_is_passthrough() {
        let mut c = Converter::new(48_000, 2, format(48_000, 2)).unwrap();
        let mut out = Vec::new();
        c.process(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn mono_is_duplicated_and_stereo_is_downmixed() {
        let mut up = Converter::new(48_000, 1, format(48_000, 2)).unwrap();
        let mut out = Vec::new();
        up.process(&[0.5, -0.5], &mut out);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);

        let mut down = Converter::new(48_000, 2, format(48_000, 1)).unwrap();
        out.clear();
        down.process(&[1.0, 0.0, 0.5, 0.5], &mut out);
        assert_eq!(out, [0.5, 0.5]);
    }

    #[test]
    fn upsampling_preserves_pitch_and_level_across_irregular_packets() {
        // 1 kHz at 44.1 kHz → 48 kHz, in MP3-sized packets (1152): no pitch or level drift.
        let mut c = Converter::new(44_100, 1, format(48_000, 1)).unwrap();
        let mut out = Vec::new();
        for chunk in sine(44_100, 1000.0, 2.0).chunks(1152) {
            c.process(chunk, &mut out);
        }
        // The last blocks stay pending: the loss is bounded by the block size.
        assert!(
            out.len() > 2 * 48_000 - 3 * RESAMPLER_CHUNK_FRAMES * 2,
            "len = {}",
            out.len()
        );

        let steady = &out[4800..];
        assert!((frequency(steady, 48_000) - 1000.0).abs() < 5.0);
        assert!(
            (rms(steady) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02,
            "rms = {}",
            rms(steady)
        );
    }

    #[test]
    fn downsampling_stereo_keeps_both_channels_in_order() {
        // Left channel = 440 Hz, right channel = silence: must stay separate after resampling.
        let left = sine(48_000, 440.0, 1.0);
        let interleaved: Vec<f32> = left.iter().flat_map(|&l| [l, 0.0]).collect();
        let mut c = Converter::new(48_000, 2, format(24_000, 2)).unwrap();
        let mut out = Vec::new();
        for chunk in interleaved.chunks(2 * 1000) {
            c.process(chunk, &mut out);
        }
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        let r: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!((frequency(&l[2000..], 24_000) - 440.0).abs() < 5.0);
        assert!(rms(&r) < 1e-3, "leak into the right channel: {}", rms(&r));
    }

    #[test]
    fn unsupported_rate_is_an_error_not_a_panic() {
        assert!(Converter::new(0, 2, format(48_000, 2)).is_err());
    }
}
