# 🎵 Music only

Should Soundfolk offer a "music only" filter (no talk, no news, no ads), and where? A study, not a spec: nothing
here is built.

**Today:** search has a Genre chip that can *select* `News` or `Talk` ([09-search-filters.md](09-search-filters.md)
§3.1), but nothing *excludes* them. The history can be filtered by text and by "Liked". The stats already compute,
per station, the share of titles that could not be split into artist and title (`unrecognised_share` in
[`db/stats.rs`](../src-tauri/src/db/stats.rs)).

---

## 1. Is there a demand?

Checked on 2026-11-06 with `/last30days` (Reddit, YouTube, Hacker News, GitHub over 30 days) plus web searches.
**Partial result:** Reddit refused every request (403) and X was not configured, so the social signal is missing.
Raw results: `~/Documents/Last30Days/internet-radio-music-only-stations-no-talk-no-ads-raw-v3.md`.

What came out:
* **The need is old and recurring, but its size is unknown.** Forum threads ask for "stations with minimal talking
  and no ads" ([MetaFilter, 2020](https://ask.metafilter.com/342451/Radio-stations-with-minimal-talking-and-no-ads),
  [Singletrack](https://singletrackworld.com/forum/off-topic/radio-station-with-no-talking/),
  [diyAudio](https://diyaudio.com/community/threads/your-favorite-internet-radio-stations.414463)). The answers
  always name the same stations: SomaFM, Radio Paradise, FIP, Folk Alley. People ask for **stations**, not for a
  feature.
* **Ads are the main complaint, more than hosts.** The two relevant GitHub items from the last 30 days are both
  about ads: [skywave #30](https://github.com/adammathes/skywave/pull/30) replaced every preset station by an
  ad-free one picked by hand, because on the web every station "forces an ad to start";
  [gladys-denon-avr #37](https://github.com/LM1LC3N7/gladys-denon-avr/pull/37) learns when a station airs its ads
  and lowers the volume meanwhile.
* **Audio speech detection exists elsewhere.**
  [TuneJourney](https://alternativeto.net/software/tunejourney-com/about) analyses the stream and switches station
  when it hears speech, with a switch to turn it off. Doable, but a niche.
* **Radio-Browser tags are known to be weak.** A review of
  [RadioDroid](https://alternativeto.net/software/radiodroid/about), another Radio-Browser client, calls its tag
  filtering "much less accurate" than other apps'.
* **The "focus / deep work" argument comes from interested parties** (radio app authors, ad-blocker vendors),
  with figures that cannot be checked.

Conclusion: a real but modest need, met today by a handful of well-known stations. Worth a cheap filter, not a
large feature.

---

## 2. Options

| Option | Where | Reliable? | Cost | Verdict |
|---|---|---|---|---|
| **Hide news / talk** | Search | Approximate: many stations have no tags, some have misleading ones | Small: client-side exclusion | **Do first** |
| **Music share per station** | Station sheet, stats | Good, but only for stations already listened to | Small: data mostly exists | **Do second** |
| **Tracks only** | History | Good: rows without an artist are jingles, ads, show names | Very small | Do, independent of the rest |
| ICY title cadence heuristic | Search | Cannot work: a station must be listened to before it can be measured | — | No |
| Server worker sampling every stream | Server | Speech/music classification fails on rap, spoken word, talk-heavy shows | High: ~50,000 streams, infrastructure | No |
| Speech detection in the engine | Player | Same, plus CPU and battery on every stream | High | No ([10-discovery.md](10-discovery.md) §7) |
| Shared, crowdsourced "music only" list | soundfolk-stations | Good once filled, empty at start | Moderation, and outside the repository's scope (donation links only) | No, for now |

### 2.1 Hide news / talk (search)
* A switch under the search box, off by default: "Hide news & talk".
* Excludes stations whose tags contain `news`, `talk`, `sport`, `sports`, `podcast`, `politics`… The list lives in
  the client, like the genres of 08 §3.1.
* Radio-Browser's API cannot exclude a tag: the filter runs client-side on each page of results.
  [`catalogSearch.svelte.ts`](../src/lib/catalogSearch.svelte.ts) already drops some stations from a page and
  counts the offset on the catalog's entries, so pagination already copes with it.
* Its wording must not promise more than it does: stations without tags stay, and a generalist FM station tagged
  `pop` stays too, hosts and ads included.

### 2.2 Music share per station (local)
* From the listener's own history: the share of **listening time** spent on a recognised track (`Artist - Title`),
  weighted by time rather than by number of titles. A one-minute jingle and a five-minute track should not weigh
  the same. Crossed with the listening sessions.
* Shown in the station sheet ("Mostly music · 92 %") and in the stats, once enough has been heard (same kind of
  floor as `MIN_TRACKS_FOR_QUALITY`).
* Limit: it measures metadata, not audio. A station that announces `Artist - Title` during its host's talk counts
  as music. Ads detected per [12-ads-detection.md](12-ads-detection.md) would come out of the music share.
* Does nothing for discovery: it only knows stations already heard.

### 2.3 Tracks only (history)
* A filter next to "Liked": hides rows with no artist (jingles, ads, show names, no metadata).
* Same criterion as `unrecognised_share`. The export follows the active filter, as it already does.
* Named "Tracks only", not "Music only": it sorts by the shape of the title, not by what is heard.

---

## 3. Not done, and why
* **Skip talk or ads automatically:** Soundfolk reports what is on air and changes nothing in it
  ([12-ads-detection.md](12-ads-detection.md) §1). Speech detection in the engine was already set aside in
  [10-discovery.md](10-discovery.md) §7.
* **A curated "music 24/7" selection** (SomaFM, Radio Paradise, FIP…): this is what users actually ask for, but it
  is editorial work to maintain. To reconsider with the Discover rows of [10-discovery.md](10-discovery.md).
* **Measuring the demand properly:** rerun `/last30days` once Reddit answers again or X access is configured.
