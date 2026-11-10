// SPDX-License-Identifier: MPL-2.0
//! Network → decoding pipeline: runs on a dedicated thread (`audio-decoder`).
//!
//! 1. `stream-download` fetches the HTTP stream into a **bounded** in-memory buffer (a live stream is endless).
//! 2. `symphonia` detects the format (MP3, AAC, Ogg/Vorbis, FLAC...) and decodes to PCM.
//! 3. [`convert::Converter`] adapts channels and sample rate to the output device.
//! 4. Samples are pushed into the ring buffer read by the cpal callback.
//!
//! If the connection drops *after* the first sound, the stream is reopened automatically ([`retry`]), for
//! as long as it takes when the network is down (sleep, Wi-Fi lost): the attempt starts as soon as
//! it is back (see [`super::network`]). A connection still open across a network change or a
//! sleep is dropped as soon as it stops receiving ([`watchdog`]), instead of waiting for the read
//! timeout.
//! After a failure, the user is told whose fault it is, **only when proven** (see
//! [`super::failure`]): no network, no Internet access, a captive portal (waited for as long as it
//! takes), or the station itself (its server stopped, the stream was removed…: retried, then given
//! up; on the very first connection, reported at once). Without proof, the player says what it
//! sees and keeps trying.

mod convert;
mod retry;
mod watchdog;

use std::io;
use std::num::NonZeroUsize;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, Url};
use rtrb::Producer;
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

use self::convert::Converter;
use self::watchdog::{Activity, Fired, NetGuard, Watchdog};
use super::failure::Failure;
use super::icy::{IcyReader, TitleCallback};
use super::loudness::StationLevel;
use super::network::NetState;
use super::output::OutputFormat;
use super::{Cancel, Reporter, ScopedHook};
use crate::http::{self, public_net};

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

pub struct Job {
    pub url: Url,
    /// The stream may only reach public addresses (`public_net`).
    pub public_only: bool,
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
        .spawn(move || retry::run(job))?;
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
///
/// `public_only`: the stream may only reach public addresses (`public_net`).
pub fn probe(
    url: &Url,
    runtime: &tokio::runtime::Handle,
    public_only: bool,
) -> Result<StreamInfo, String> {
    // Packets skipped before giving up (truncated stream start, ID3 headers...).
    const MAX_PACKETS: usize = 64;

    if public_only {
        public_net::check(url)?;
    }
    let client = stream_client(public_only)?;
    let cancel = Arc::new(Cancel::default());
    let mut opened = match open_stream(
        client,
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
        if let Ok(decoded) = opened.decoder.decode(&packet)
            && decoded.frames() > 0
        {
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
    /// Stop cancels this connection's download, unblocking a read waiting for data.
    _cancel_hook: ScopedHook,
}

/// The client streams are read with, built once for each value of `public_only`.
fn stream_client(public_only: bool) -> Result<&'static Client, String> {
    static OPEN: OnceLock<Result<Client, String>> = OnceLock::new();
    static PUBLIC: OnceLock<Result<Client, String>> = OnceLock::new();
    let build = move || {
        // Request ICY metadata; a server that does not support it simply ignores the header.
        let mut headers = HeaderMap::new();
        headers.insert("Icy-MetaData", HeaderValue::from_static("1"));
        let builder = http::builder()
            .user_agent(http::STREAM_USER_AGENT)
            .default_headers(headers)
            // The stream is read as raw bytes (ICY blocks, range requests): never compressed.
            .no_gzip()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            // The client outlives sessions: a connection kept from one would be dead after a
            // network change. Each stream opens its own, as when the client was built per session.
            .pool_max_idle_per_host(0);
        if public_only {
            public_net::restrict(builder)
        } else {
            builder
        }
    };
    http::cached(if public_only { &PUBLIC } else { &OPEN }, build)
}

/// Connection, network buffer, ICY metadata stripping, format detection, decoder creation.
fn open_stream(
    client: &Client,
    url: &Url,
    runtime: &tokio::runtime::Handle,
    cancel: &Arc<Cancel>,
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
    // Kept with the stream, dropped with it: a reconnection registers its own.
    let cancel_hook = cancel.on_cancel_scoped(move || token.cancel());
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
        _cancel_hook: cancel_hook,
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
        _cancel_hook,
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
