# 🎵 Audio Engine & Facade Architecture (`AudioEngine`)

## 1. Philosophy and design
Sound handling is the critical core of Soundfolk. The architecture is a **facade**: the implementation is fully
isolated per target platform, while the Rust backend and the Tauri interface see one unified API.

```rust
pub trait AudioEngine {
    /// Hands the station to the engine and returns at once; `PlayReply::wait` blocks until the
    /// sound output is open (or failed to), so callers wait off the async runtime and without
    /// holding a lock. `station` (its Radio-Browser UUID) helps find the title of streams that do
    /// not announce it.
    fn play(&mut self, url: &str, station: StationRef) -> Result<PlayReply, String>;
    /// On a live stream, pausing mutes the sound but keeps consuming the stream,
    /// so `resume` picks up on air, not where the pause happened.
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    /// `volume` is the slider position, clamped to `0.0..=1.0`; the gain applied is its cube
    /// (a perceptual curve close to a 60 dB fader, as PulseAudio / PipeWire do).
    fn set_volume(&mut self, volume: f32) -> Result<(), String>;
}
```

The engine does not depend on Tauri: player state and now-playing information are published through listeners that
the application layer plugs into its event bus (`player:state`, `player:now-playing`).

## 2. Desktop implementation (Linux MVP, Windows, macOS)
Desktop uses a 100 % pure-Rust stack:

* **`stream-download`**: HTTP client with a bounded in-memory buffer (a live stream is endless, so storage must not
  grow). A small prefetch (16 KiB) keeps start-up fast: Icecast servers send a burst of roughly 30 KiB on connect,
  then deliver at playback speed, so waiting for more would only delay the first sound.
* **`symphonia`**: modular decoding of MP3, AAC (ADTS & MP4), Ogg/Vorbis, FLAC and WAV, with format auto-detection.
* **`rubato`**: high-quality FFT resampling when the stream's sample rate differs from the device's (bypassed when
  they match); channel remapping is done before it.
* **`cpal`**: low-level output to the sound card. The device's default format is used; the decoder adapts to it, never
  the other way round.
* **`ebur128`**: loudness of the stream (ITU-R BS.1770) for *Constant volume*, which applies one slowly moving gain
  per station on the decoder thread ([11-loudness.md](11-loudness.md)).
* **`rtrb`**: lock-free ring buffer (2 s) between the decoder thread and the real-time audio callback, which only
  pops samples, applies the volume and converts to the device sample type.

```
HTTP ─▶ stream-download ─▶ ICY filter ─▶ symphonia ─▶ convert ─▶ loudness ─▶ ring buffer ─▶ cpal ─▶ sound card
        (bounded buffer)   (titles)      (decode)     (rubato)   (station     (SPSC)         (callback,
                                                                  gain)                       × volume)
        └──────────────────── "audio-decoder" thread ────────────────────┘                 └ audio thread
```

### Behavior
* **Start-up:** playback starts after a 120 ms buffer. After an underrun the engine waits for 1 s of audio before
  resuming, so a network hiccup does not turn into a stutter.
* **Resilience:** if the stream drops *after* the first sound, it is reopened automatically (state `reconnecting`),
  with exponential backoff capped at 30 s. A failure on the very first connection is reported immediately (dead URL,
  unknown format…), unless the network is down.
* **Sleep and network changes:** a watcher (`audio/network.rs`, one check per second **only while a station plays**,
  parked otherwise) follows the local address of the default IPv4 / IPv6 routes — it changes on Wi-Fi ↔ Ethernet ↔ 4G
  switches, VPNs, new DHCP leases, and disappears when the network is lost — and detects a wake from sleep: the wall
  clock jumps ahead of the monotonic one on Linux and macOS; on Windows, where the monotonic clock keeps counting
  during sleep, through the system's resume notification (`PowerRegisterSuspendResumeNotification`). A drop shorter
  than a second with the same address coming back goes unseen; the read timeout catches it. Then:
  * a connection that receives nothing for 3 s after a change or a wake is dropped and reopened at once, instead of
    waiting for the 8 s read timeout; one that keeps receiving is kept (Ethernet → Wi-Fi with both up does not cut
    the sound);
  * after a failure, the player says **whose fault it is, only when proven** — a wrong culprit misleads more than
    none (`audio/failure.rs`, `audio/connectivity.rs`):
    * *No network — waiting…*: no route at all;
    * *No Internet access — waiting…*: two independent signals agree — Soundfolk's own check fails (no HTTPS answer
      from the Radio-Browser API, already used for the catalog, so no new third party) **and** the operating system
      says there is no Internet (Windows: `GetNetworkConnectivityHint`; Linux: the desktop portal or
      NetworkManager, which run their own checks; macOS: Network.framework's path monitor, which only knows "no
      usable path at all", so a network without Internet stays unnamed there). Either alone can be wrong
      (Radio-Browser down or blocked, a network NetworkManager does not manage, a proxy that fools Windows);
    * *Sign in to the network (captive portal) — waiting…*: the system says so, or the network answered HTTP 511;
    * *The station is not broadcasting (reason) — retrying…*: proven — its address answered with an HTTP client
      error, or Internet works (Radio-Browser answers). Reasons in plain words: its server refuses connections, does
      not answer, its address no longer exists, the stream is not on air (HTTP 404: on Icecast, also a mount
      whose source is off air), the stream no longer exists (HTTP 410), its server has a problem
      (HTTP 5xx), the stream stopped, it does not send playable audio;
    * *Cannot play the station (what was seen) — retrying…*: no proof either way (e.g. macOS, which has no system
      check, or the signals disagree): facts only — "its server's address cannot be found", not "no longer exists".

    Without network or Internet, and without proof, the engine waits as long as it takes and reconnects as soon as
    the network comes back or changes. A station proven at fault is retried, then given up after 5 minutes (*The station stopped broadcasting: reason*); on
    the very first connection it is reported at once (*The station is not broadcasting: reason*);
  * a sound output that dies (device gone, sound server restarted, callback silent for 5 s) is reopened, keeping the
    pause and the title.
  * the listening session ends when the sound stopped, no later than 90 s after its last heartbeat: time asleep is not
    listening time.
* **Cancellation:** stopping also cancels the network download, so a blocked read is released and `stop` / station
  changes take effect immediately.
* **ICY metadata:** the engine asks for `Icy-MetaData: 1`, strips the metadata blocks before decoding and publishes the
  station name (`icy-name`) and the current title. Streams that do not advertise `icy-metaint` (e.g. Ogg) have no
  title.
* **Playlists & probing:** a user-supplied URL can be a direct stream, a `.m3u` or a `.pls`; it is resolved to the real
  stream and then **probed** (connection, format detection, first decoded packet) without playing anything.
* **Not supported yet:** HLS (`.m3u8`).

## 3. Mobile implementation (future — iOS & Android)
To guarantee background playback, call-interruption handling and control from the lock screen or Bluetooth
headsets:

* **iOS** (`#[cfg(target_os = "ios")]`): delegation to Apple's native `AVPlayer`.
* **Android** (`#[cfg(target_os = "android")]`): delegation to Google's native `ExoPlayer` (AndroidX Media3).
