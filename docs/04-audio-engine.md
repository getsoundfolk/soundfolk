# 🎵 Audio Engine & Facade Architecture (`AudioEngine`)

## 1. Philosophy and design
Sound handling is the critical core of Soundfolk. The architecture is a **facade**: the implementation is fully
isolated per target platform, while the Rust backend and the Tauri interface see one unified API.

```rust
pub trait AudioEngine {
    fn play(&mut self, url: &str) -> Result<(), String>;
    /// On a live stream, pausing mutes the sound but keeps consuming the stream,
    /// so `resume` picks up on air, not where the pause happened.
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    /// `volume` is clamped to `0.0..=1.0`.
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
* **`rtrb`**: lock-free ring buffer (2 s) between the decoder thread and the real-time audio callback, which only
  pops samples, applies the volume and converts to the device sample type.

```
HTTP ─▶ stream-download ─▶ ICY filter ─▶ symphonia ─▶ convert ─▶ ring buffer ─▶ cpal ─▶ sound card
        (bounded buffer)   (titles)      (decode)     (rubato)    (SPSC)         (callback)
        └──────────── "audio-decoder" thread ───────────────────┘              └ audio thread
```

### Behavior
* **Start-up:** playback starts after a 120 ms buffer. After an underrun the engine waits for 1 s of audio before
  resuming, so a network hiccup does not turn into a stutter.
* **Resilience:** if the stream drops *after* the first sound, it is reopened automatically (up to 5 attempts with
  exponential backoff). A failure on the very first connection is reported immediately (dead URL, unknown format…).
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
