# 🔊 Constant Volume (loudness normalization)

Spec for keeping the same perceived level from one station to the next, so the listener never has to touch the volume
after switching.

**Today:** built on desktop (`audio/loudness.rs`, `db/loudness.rs`, `volume.rs`), on by default. Target, guess and
time constants are tuned on 207 stations (§6.1), not yet checked by ear.

---

## 1. What listeners ask for

Same caveat as [09-search-filters.md](09-search-filters.md) §1: a synthesis of feedback on radio players (TuneIn,
Radio Garden, Radio-Browser apps, forums), not data.

* **The complaint:** "every time I change station I have to fix the volume". One station blasts, the next is barely
  audible. Gaps of 6 to 10 dB and more are common: some stations are mastered and compressed hard (around −8 LUFS),
  others stream a raw, quiet feed (−20 LUFS and below).
* **Who it hurts most:** people who switch often — Surprise me and Scan ([10-discovery.md](10-discovery.md)), media
  "next" on headphones or in the car, listening at night or in the background. Those who stay on 3–4 favorites get
  used to their levels; it matters less to them, but never hurts.
* **The worst moment is the first second:** a station much louder than the previous one makes the listener jump.
  A normalization that needs 5 s to react misses the case people remember.
* **What they do *not* ask for:** exact EBU R128 compliance, settings, or a processing that flattens the music.
  "Why do music apps have it and radio apps don't?" — they want a *Constant volume* switch, and to forget it.

When it works, nobody notices it; that is the goal.

---

## 2. Principles

1. **Reduce the big gaps, not the small ones.** Bringing every station within ±2 dB of a target is enough; chasing the
   last decibel means a fast gain that pumps.
2. **A gain, never a compressor.** One slowly moving gain per station keeps the dynamics the station chose. No
   multiband, no per-transient processing.
3. **Never surprise loud.** Cutting is fast, boosting is slow and capped. A wrong cut costs a few seconds of quiet
   sound; a wrong boost hurts.
4. **Never clip.** A boost is limited by the headroom actually measured (§3.4); there is no limiter to hide a bad
   decision.
5. **Learn and remember.** A station heard once starts at the right level next time (§3.3). This is what fixes the
   first second.
6. **The slider stays the listener's.** Normalization moves the station towards the target; the slider then sets the
   level as before. They multiply; neither moves the other.

---

## 3. Design (desktop engine)

### 3.1 Where in the pipeline

```
symphonia ─▶ convert ─▶ loudness meter ─▶ × station gain ─▶ ring buffer ─▶ cpal callback (× slider volume)
 (decode)    (rubato)   (K-weighted)       (ramped)          (2 s)
└───────────────────── "audio-decoder" thread ──────────────────────┘     └ audio thread
```

Measurement and station gain run on the **decoder thread**, after conversion and before the ring buffer:

* the real-time callback stays as it is (pop, × volume, convert): nothing heavier on the audio thread;
* the gain is applied to the very samples just measured, with a per-sample ramp, so no zipper noise and no extra
  atomics between threads;
* the slider stays in the callback, so moving it remains immediate despite the 2 s buffer.

The engine's `Controls` get one more flag, `normalize: AtomicBool`, read by the decoder thread for each packet; turning
the switch off (or on) moves the station gain at 15 dB/s, so it gets there within ~1 s (no jump). The meter keeps
measuring while the switch is off, so turning it on applies a known level at once.

### 3.2 Measuring

* **Loudness, not peak or RMS:** ITU-R BS.1770 K-weighting (the filter behind LUFS), so a bass-heavy station is not
  judged louder than it sounds. Crate: [`ebur128`](https://crates.io/crates/ebur128) (pure Rust, MIT/Apache, a port
  of libebur128), modes `S` (short-term, 3 s window) and `SAMPLE_PEAK`. Cost: two biquads per channel and a running
  sum, negligible next to decoding.
* **Gating:** blocks below −50 LUFS (silence, gaps between tracks, a stream that stalls) are ignored, as in R128's
  absolute gate. A relative gate (−10 LU below the running level, as R128) keeps a quiet talk segment from pulling the
  gain up between two songs.
* **Running estimate:** an exponential average of the gated short-term loudness, in the dB domain, with an asymmetric
  time constant:

  | Situation | Time constant | Why |
  |---|---|---|
  | Learning: first 10 s of an unknown station | ~1 s, both ways | The error is our guess (§3.3), not the station's dynamics |
  | Louder than the estimate | ~4 s | Cut quickly (principle 3) |
  | Quieter than the estimate | ~40 s | A quiet song or a talk break must not pump the gain up |

  While learning, the first value comes from the **momentary** loudness (400 ms window) gated over the first 1.5 s,
  not from the 3 s short-term window: zapping (Scan, media "next") hears each station for a few seconds only.

  A fast rise on so little audio can be fooled by a very quiet start (a whispered spoken intro): the gain would jump
  up, then come down at the first song. So while learning, the gain stays **at most +3 dB** (instead of +6); the
  rest of the boost comes after, at the slow rate, once the estimate rests on more audio. Cuts are not limited.
  Louder / quieter were 2 s / 20 s at first; measured on 207 stations (§6.1), that moved the gain twice as much once
  settled, for nothing.

### 3.3 The gain, and the first second

`station gain (dB) = target − estimate`, clamped to **[−15 dB, +6 dB]** (+3 dB while learning, §3.2), and to the
headroom rule of §3.4.

* **Target: −16 LUFS** (between Apple Music's −16 and the −14 of Spotify / YouTube). Most stations are louder:
  normalization mostly *cuts*, which is the safe direction. The listener raises the slider once, and never again.
  The cost: with normalization on, everything sounds a few dB quieter than the same slider position off, and than
  Spotify at the same system volume. Measured (§6.1): the median station is at −12 LUFS, so the typical cut is 4 dB.
  −14 would narrow that gap, but stations peak near 0 dBFS, so the boost a quieter target asks for is mostly
  refused by the headroom rule: 32 stations of 207 would stay quieter than the rest, against 11 at −16.
* **Known station:** its last estimate is read from the database (§4) when the playback starts (the engine asks its
  `LevelMemory`, set by the app); the gain applies from the first sample, then follows the live measure.
* **Unknown station:** starts at the **median loudness of the stations already learned** (§4), once there are at
  least 20 of them; before that, at **−11 LUFS** (−5 dB), 1 dB above the measured median (§6.1): on the quiet
  side, as a guess too loud is worse.
  The median, not a low percentile: a guess too low makes the listener raise the slider, then the station comes up
  and ends too loud — the very jump this feature removes. The learning phase (§3.2) then corrects it within ~2 s,
  up or down.
* **Station change:** the new gain is set before the first sample of the new stream; nothing carries over from the
  previous station.
* **Reconnection** (same station, `reconnecting`): the estimate is kept, as it is the same stream.
* **Many unknown stations in a row** (Surprise me, Scan): each starts at the median guess and is corrected within
  ~2 s; 10 s of gated audio are enough to store it (§4), so a 15 s Scan step teaches the level too.
* **The listener moves the slider during a correction:** nothing special. Slider and station gain multiply and
  neither moves the other (principle 6). Coupling them (freezing the gain while the slider moves, or reading a slider
  move as "this station is too quiet") would make the result depend on what the listener did during the first
  seconds; the short learning phase keeps that window small instead.

### 3.4 Headroom: boosting without clipping

Sample peak over the last ~10 s is tracked with the loudness. A boost is capped so that `peak × gain ≤ −1 dBFS`.
A quiet but already peaking stream (a raw feed with loud jingles) is therefore left quieter: better than distorted.
Cuts are never limited.

No limiter in the first version: it would let the gain go higher at the price of a processing that changes the sound,
against principle 2. Measured: it would change little (§6.1).

### 3.5 Ramps

Every gain change is a linear ramp in dB over at least 50 ms, and the gain moves at most 6 dB/s once playing (fast
cut) and 0.5 dB/s up, except while learning (§3.2), where it may rise at 6 dB/s too. Pause keeps the gain; the stream
keeps being consumed, so the measure keeps running too.

---

## 4. Storage

One row per station, its loudness rather than the gain, so changing the target never invalidates what was learned:

```sql
CREATE TABLE IF NOT EXISTS station_loudness (
    station_key   TEXT PRIMARY KEY,  -- Radio-Browser UUID, else the stream URL (personal stations)
    loudness_lufs REAL NOT NULL,     -- running gated estimate (§3.2)
    peak_dbfs     REAL NOT NULL,     -- recent sample peak (§3.4)
    measured_s    REAL NOT NULL,     -- seconds of gated audio behind the estimate, capped at 1 h
    updated_at    INTEGER NOT NULL
);
```

* Written when the station stops or changes, and every 60 s while it plays, only after 10 s of gated audio (a station
  heard for 2 s teaches nothing).
* On a later listen, the stored value is the starting point; the more seconds behind it (`measured_s`, full weight at
  10 min), the slower the live measure lowers it during the first minute (up to 3× the time constant). Only lowering:
  a station that got louder is followed at the usual speed (principle 3).
* A station whose stream changes (new URL, other bitrate) keeps its row when keyed by UUID: the level is the station's
  mastering, rarely the codec's.
* **No expiry.** The row is a running estimate, updated at every listen, so a station that changes its processing is
  followed within a minute of listening. The one wrong moment is the start of the first listen after the change; the
  fast cut covers the dangerous direction. If the live estimate stays more than 6 dB away from the stored one for 30 s,
  `measured_s` is reset: the live measure takes over at once instead of being slowed down by an outdated history.
* Cleared with the history ("clear history" also forgets levels). Never leaves the device.

---

## 5. Interface

* **⚙ Settings → Playback: "Constant volume"** — *Keeps the same level from one station to the next.* One switch,
  **on by default**: it is what nearly every listener wants, and its failure mode (a station a bit quieter than its
  own mastering) is mild.
* Nothing in the player. The station sheet shows, in its details under the stream address, *Volume: Level
  adjusted: −7 dB* (read every 2 s while the sheet is open, only while the station plays and the switch is on), for the
  curious and for bug reports. The sheet has no folded section yet; if it gets one, the line moves there.
* No per-station override in the first version (§6).

---

## 6. Measurements and open questions

### 6.1 Measured on 207 stations (2026-11-05)

The 220 most clicked Radio-Browser stations, at most 25 per country and one per name (FR, US, DE 25 each, TR 20,
IN 17, IT and GB 15…; 178 MP3, 41 AAC), each recorded for 90 s with `ffmpeg`'s R128 meter (momentary, short-term
and sample peak every 100 ms); 207 delivered enough sound. The leveler was then replayed on these series, each
station as unknown, for each setting. Scripts and station list in [loudness-study/](loudness-study/):
`stations.py` picks them, `measure.sh` records one, `sim.py` replays the leveler and prints what follows.
`cargo test -- --ignored live_loudness --nocapture` (with `SOUNDFOLK_STREAMS`) checks the real engine on a few.

**Loudness of the stations** (integrated, LUFS):

| p10 | p25 | median | p75 | p90 | quietest | loudest |
|---|---|---|---|---|---|---|
| −17.6 | −15.2 | −12.2 | −9.6 | −7.7 | −23.4 | −3.6 |

Without normalization, 80 % of stations sit within −5.4 / +4.5 dB of the median: a 10 dB range from one switch to
the next. With it, 80 % end within −1.3 / 0 dB of the target. Stations peak high: the median of each station's
highest sample peak is −0.9 dBFS (MP3 and AAC decoding often go past 0), so the headroom rule (§3.4) refuses most
boosts. At −16 LUFS, 41 stations need one and 11 stay more than 1 dB quieter; without the rule, 9. A limiter
would not help much.

**Settings compared** (each row changes one thing from the first):

| Setting | Gain moves once settled | Pumping range (p90) | Settled after (median) | Ends within (p10..p90) | Left quiet |
|---|---|---|---|---|---|
| 2 s / 20 s, −16, guess −11 (first version) | 4.0 dB/min | 4.0 dB | 34 s | −1.2..−0.1 dB | 11 |
| **4 s / 40 s (kept)** | **2.2 dB/min** | **3.3 dB** | **19 s** | **−1.3..0.0 dB** | **11** |
| 8 s / 40 s | 1.9 dB/min | 3.2 dB | 18 s | −1.3..+0.2 dB | 8 |
| 2 s / 60 s | 2.0 dB/min | 3.2 dB | 18 s | −1.8..−0.2 dB | 13 |
| target −14 | 3.1 dB/min | 3.6 dB | 25 s | −1.9..−0.1 dB | 32 |
| target −18 | 4.0 dB/min | 4.3 dB | 34 s | −1.1..−0.1 dB | 4 |

* **Time constants:** halving how much the gain moves costs nothing measurable; slower still (8 s to cut) gains
  little more and lets a louder passage through longer. 4 s / 40 s it is.
* **Target:** −16 kept (§3.3). −18 leaves fewer stations quiet but lowers everything 2 dB more.
* **Starting guess and first value:** a guess of −9 / −10 / −11 LUFS trades how loud the first seconds can be against
  how quiet: at −11, 10 % of unknown stations start more than 5 dB louder than where they settle (for ~2 s), and 10 %
  about 6 dB quieter. A lower guess makes the first case rarer and the second one worse: kept at −11 (§3.3).
  Waiting 3 s instead of 1.5 s for the first value, or a learning time constant of 0.5 or 2 s, changes it by less
  than 1 dB: kept.

### 6.2 Open questions

* **By ear:** the measures say how much the gain moves, not whether it is heard. To check on stations mixing music
  and talk (news, morning shows), where pumping would show first, and while zapping.
* **Very dynamic stations** (classical, jazz, ambient): one slow gain leaves their dynamics alone, which is wanted.
  Check that the relative gate keeps a long quiet movement from lifting the gain.
* **Ads and jingles** are often mastered louder than the programme. The fast cut lowers them, then the slow rise takes
  ~40 s to come back: acceptable, or should a short loud burst (< 30 s) be ignored by the long-term estimate?
* **Mono and speech-only stations:** K-weighting judges them fairly, but a talk station at −16 LUFS can feel louder
  than music at the same level. Leave as is unless people complain.
* **Per-station manual offset** (±6 dB in the station sheet): only if people ask after it ships.
* **Limiter:** see §3.4.

---

## 7. Mobile (future)

`AVPlayer` and ExoPlayer do not run our decoder, so §3 does not apply as is:

* **Android:** an `AudioProcessor` in ExoPlayer's pipeline can host the same meter and gain (same Rust code through
  JNI, or a Kotlin port). `LoudnessCodecController` (Android 15) only applies to codecs carrying loudness metadata,
  which streams do not.
* **iOS:** an `MTAudioProcessingTap` on the `AVPlayerItem` can measure and apply the gain.

The stored levels (§4) are shared, so a station learned on desktop starts right on mobile once favorites sync.

---

## 8. Changes

**Backend** (`audio/`, `db/`)
- [x] `ebur128` dependency; `audio/loudness.rs`: gated meter, asymmetric estimate, gain with clamps, headroom rule
  and ramps (§3.2–3.5), with no engine dependency so it is unit-testable.
- [x] Decoder thread: meter + gain between `convert` and the ring buffer; `normalize` flag in `Controls`.
- [x] The engine asks a `LevelMemory` (the database, set by the app) for the station's stored loudness and the
  guess when a playback starts, and gives it the level on stop and every 60 s.
- [x] `station_loudness` table, read and write, cleared with the history.
- [x] Setting `constant_volume` (default on), command to set it, applied at start-up.
- [x] Starting guess: median of `station_loudness` once it has 20 rows, else −11 LUFS; reset of `measured_s` when the
  live estimate stays 6 dB away for 30 s.
- [x] Tests: a sine at −8 / −16 / −26 LUFS converges to the target ±1 dB within the stated times (±1 dB within 2 s
  while learning); silence and a quiet segment do not raise the gain; a peaking quiet signal is not boosted past
  −1 dBFS; a quiet 5 s intro before a loud signal never takes the gain above +3 dB; a stored level gives the right
  gain on the first sample; switching off ramps back to 0 dB; a station that changed drops its history.
- [x] `cargo test -- --ignored live_loudness`: levels learned on real stations.
- [x] Measure ~200 stations and tune target, guess and time constants (§6.1).
- [ ] Listen: pumping on music + talk stations, first seconds while zapping (§6.2).

**Frontend**
- [x] "Constant volume" switch in ⚙ → Playback (English only, like the rest of the UI for now).
- [x] "Level adjusted" line in the station sheet's details.

**Docs**
- [x] [04-audio-engine.md](04-audio-engine.md): the meter in the pipeline diagram.
