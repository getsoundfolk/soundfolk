# 📢 Ads detection

How Soundfolk could tell that an ad is on air, and what it would do with it. Not built yet: this is the plan.

## 1. Goals
* **Show it.** While an ad plays, the player shows "Ad" instead of a track. If the length of the ad is known, it
  also shows a countdown.
* **Keep it out of the listener's data.** An ad is not written to the history, is not counted in the stats, and
  starts no cover lookup (MusicBrainz, Cover Art Archive) or any other online query about the track.
* **On every station.** Each station gets the best signal it offers, from the list in section 2. A station with
  no signal at all plays as today.

What it does not do: mute, skip or cut ads. Soundfolk reports what is on air and nothing more. The ad is part of
how the station pays for its stream.

## 2. Signals, from the most to the least reliable

### 2.1 Explicit markers in the stream (verified)
AdsWizz is an ad server that inserts ads into the stream on the server side. When it does, the ICY metadata block
carries keys prefixed `adw_` next to an empty `StreamTitle`. Seen on 2026-11-05 on a RadioKing station
(`listen.radioking.com/radio/869771/…`), on connect:

```
StreamTitle='';StreamUrl='';adw_ad='true';durationMilliseconds='30119';adId='0';insertionType='preroll';
```

* `adw_ad='true'`: an ad is playing.
* `durationMilliseconds`: its length, which gives the countdown.
* `insertionType`: `preroll` means on connect. A `midroll` value, for an ad inserted mid-stream, is likely but has
  not been seen yet.
* The ad ends at the next metadata block with a real `StreamTitle`. On that station, the title came back 26 s
  after the ad block.

Only 1 station in 24 RadioKing stations sent this, once, on connect. The share of stations that do it is not
known. AdsWizz serves the stream behind the platform's own URL, so the station list in Radio-Browser cannot tell
these stations apart. Measuring it is the first task (section 4).

Today [`icy.rs`](../src-tauri/src/audio/icy.rs) reads only `StreamTitle` and returns `None` when it is empty, so
the rest of the block is dropped. The first change is to parse the whole block into its keys and pass the ad keys
on.

### 2.2 Ad markers in HLS playlists (to probe)
HLS streams can mark ad breaks in the playlist (`#EXT-X-CUE-OUT` / `#EXT-X-CUE-IN`, `#EXT-X-DATERANGE` with
SCTE-35) or in ID3 tags inside segments. That is common practice for HLS ads, but none of the 4,053 `.m3u8`
stations in Radio-Browser has been checked. It goes with ID3 timed metadata, which is already a TODO in
[07-now-playing.md](07-now-playing.md).

### 2.3 Platform APIs (to probe)
* **Triton**: the now-playing endpoint is called with `eventType=track` (see 07-now-playing.md). Other event
  types are possible, and ads may be one of them: to check.
* **AzuraCast**: an ad is usually a playlist of its own. The playlist name in `/api/nowplaying` could be used, but
  only with a list of names, which is a heuristic (2.4).
* **RadioKing** `track/current`: no ad field seen. During the preroll above, the API kept describing the track
  playing for listeners who were already connected.

### 2.4 The title text (heuristic)
Titles that announce an ad: "Publicité", "Pub", "Advertisement", "Commercial", "Werbung", "Ad break", and the
like. Rules, to avoid false positives:
* Match the whole title (after `tracks::parse` cleanup), never a word inside it. "Pub Crawl" is a song.
* The list lives in data, not in code, so it can grow without a release.
* A title that is only the station's name, or a slogan, is a jingle, not an ad. Jingles are another subject (they
  already pollute the history) and are not handled here.

### 2.5 No signal
Many stations send nothing during ads: the previous title stays, or the title is empty. Recognizing an ad from the
audio itself (fingerprints, silence detection) is out of scope.

## 3. Behavior

### 3.1 The player
* An ad is a state of the track on air, next to "title" and "no title": the frontend gets `ad: { ends_at:
  Option<time> }` instead of a track.
* It shows "Ad". With a length, it adds a countdown that starts when the marker is **heard**, not when the server
  says the ad began (after the preroll above, that listener heard each title change 21 to 29 s after the start the RadioKing API
  gave for it).
* No countdown below zero. If the length runs out and no title follows, it shows "Ad" with no time until the next
  title.
* The media controls (MPRIS / SMTC) and notifications show the station name, not "Ad" as a track, and send no
  notification for it.

### 3.2 History, stats, covers
* `journalize_title` ([lib.rs](../src-tauri/src/lib.rs)) writes nothing for an ad, and the track that comes after
  it is logged as usual.
* No cover lookup ([covers.rs](../src-tauri/src/covers.rs)) and no MusicBrainz query for an ad.
* A title detected by the heuristic (2.4) gets the same treatment. Because that can be wrong, the station sheet
  says which signal matched.

### 3.3 Progress bar
When an API gives the track's start and length (AzuraCast, RadioKing), an ad heard on connect explains why the
listener is behind the API's clock. The bar is anchored on the moment the title is heard anyway (see the RadioKing
measures in 07-now-playing.md), so the ad needs no special case beyond that.

### 3.4 Station sheet
"Track on air" says how ads are detected on this station: "Ads announced by the stream (AdsWizz)", "Ads recognized
from their title", or nothing.

## 4. Order of work
1. **Measure.** Connect several times to a few dozen stations per platform (RadioKing, Triton, Zeno, laut.fm,
   Radio.co, a sample of the rest), log every metadata block, and count `adw_` keys, prerolls and midrolls.
2. Parse every key of the ICY block in `icy.rs`, carry the ad marker to the reporter, and add the "Ad" state to the
   track on air.
3. Keep ads out of the history, the stats and the cover lookup.
4. The "Ad" display and its countdown.
5. Then, depending on what step 1 finds: HLS markers, Triton event types, the title heuristic.
