# 🧭 Discovery

Spec for finding stations the listener was not looking for: the **Discover** destination of
[06-ui-principles.md](06-ui-principles.md) §1, beyond search and filters
([09-search-filters.md](09-search-filters.md)).

**Today:** the app has three tabs, Listen, Discover and History. Discover has the search box, **Surprise me** (§4.1,
`catalog/surprise.rs`) and one row per curated list (§5.2); the search puts the matching lists first. Not built: the
chips of §3 (search still sends only the typed text, 09), Scan (§4.2), rule collections (§5.1), hidden gems (§5.3)
and More like this (§6).

---

## 1. What listeners ask for

Same caveat as 08 §1: a synthesis of feedback on radio players (Radio Garden, Transistor, Shortwave, TuneIn, radio.net,
Reddit threads), with no sources cited. A ranking of hypotheses, not data.

Two profiles, often the same person on different days:

| Profile | Wants | Finds stations through |
|---|---|---|
| **Explorer** | Chance, the unexpected, "travelling" | Random, a globe, scanning the dial |
| **Targeted listener** | Something precise (genre + country + quality) | Filters, search, curated lists |

Targeted listeners are served by 08. This document is for explorers, who are the most attached to web radio: they
describe Radio Garden as "addictive" and as the way out of their streaming bubble. Their complaints, in order:

1. **Random lands on bad stations**: dead streams, poor quality, generic commercial radio.
2. No way to combine language + genre + country while exploring.
3. Not enough curation.
4. No automatic **scan** like the dial of an old tuner.

The strongest reaction comes from **controlled chance**: random, with a quality floor. Daily use still rests on
filters and favorites.

---

## 2. What the catalog gives

Measured on 500 stations drawn with `order=random&hidebroken=true` (`de1.api.radio-browser.info`, 2026-11-05), HLS
dropped (468 left):

| Property | Share |
|---|---|
| No tag | 35 % |
| Bitrate unknown (`0`) | 16 % |
| Bitrate known but < 128 kbit/s | 22 % |
| No click in the last 24 h (`clickcount` = 0) | 42 % |
| No vote | 9 % |
| No favicon | 37 % |
| Tags **and** ≥ 64 kbit/s **and** a homepage | 47 % |

So a plain random draw is "bad" about half the time before a single sound is played. The catalog cannot filter on
tags being present, on a homepage, or on a minimum click count: those are client-side checks on a random batch.

The station object has **no creation date** (`lastchangetime` is the last edit, `changeuuid` changes with every edit).
"Recently added" cannot be built honestly from the catalog; see §7.

The catalog has **no notion of ad-free or listener-supported** stations (06 §3 and §5 use both as examples): they
need our own data (§5.2).

---

## 3. The Discover screen

One screen, one level (06 §1), never empty (06 §3). From top to bottom:

1. **Surprise me** — the main button, with the 08 chips next to it (Genre, Country, Language). The chips are
   shared with search: what is selected in one is selected in the other. They filter the rows too: a row stays when
   its collection or list matches. **Use**, the chip only curated lists have
   ([15-curated-lists.md](15-curated-lists.md) §6.5), comes once there are about ten lists: with four rows all
   visible, it filters nothing.
2. **Scan** — next to Surprise me (§4.2).
3. **Collections** — horizontal rows of stations, one per collection (§5): personal ones first, then the curated
   lists (§5.2), then the rule collections. A list's row shows its title, "by @login" and one line of description.
4. The search box, which turns the screen into the search results of 08 as soon as text is typed: the matching
   curated lists first, as one row, then the stations.

The station sheet gets **More like this** (§6).

---

## 4. Chance

### 4.1 Surprise me
One tap plays a station drawn at random among those matching the active chips. Built on `surprise_station(filters)`
(§8), with a quality floor the listener does not have to set:

* **Catalog side:** `order=random`, `hidebroken=true`, `limit=20`.
* **Client side**, on that batch, keep a station only if:
  * it is not HLS, not a duplicate (same rules as search);
  * it has at least one tag (a station without one is also one we cannot describe in a row, 06 §5);
  * its bitrate is unknown or ≥ 64 kbit/s;
  * the listener has not heard it in the last 30 days and it is not a favorite — explorers want the new;
  * it was not drawn earlier in this session.
* **Order of the survivors:** those with a homepage and a favicon first (a sign someone maintains the entry), then the
  rest. Within each group, the catalog's random order. Never by click count: it would turn chance into "Popular".
* **Play the first one that actually plays.** The catalog check gives false negatives and false positives; the real
  test is the stream. No sound within 5 s → next survivor, silently. Batch exhausted → a new request, at most three
  in a row, then one sentence: "Nothing playable right now with these filters", with a button removing the most
  restrictive one (08 §5).
* While a surprise plays, the player shows **Another one** (and media "next" does the same): the next survivor,
  without a new request when the batch still has some.

No chips selected is fine: the whole catalog, same floor.

**Built so far** (a small button beside the search box, which comes first in Discover, `catalog/surprise.rs`, `player.surprise()`): a random draw of 60
stations the catalog found up, kept only when their tags name a kind of music and none of talk (news, talk, sport,
podcast, religion…), not HLS, bitrate unknown or ≥ 64 kbit/s; about a third of a draw passes. Maintained entries
first, then each candidate gets 2 s to be heard (working streams measured at 0.6 to 1 s), and is dropped at once if the engine reports a failure or starts
reconnecting. Three draws at most. Not yet: chips, the history and favorites
exclusions, Another one.

### 4.2 Scan
The tuner of an old radio: Soundfolk changes station on its own every few seconds until the listener stops on one.

* Each step is a Surprise me draw with the current chips.
* Interval: **15 s** of sound, counted from the first decoded audio, not from the request (a slow stream must not eat
  its own turn). Choice of 10 / 15 / 30 s on the scan bar itself, remembered; not in ⚙.
* The scan bar replaces the Surprise me button while scanning: interval, **Keep** (stops scanning on this station),
  **Next** (skips now), **Stop** (stops and goes back to the previous station, as a tuner returns to its preset).
* Scanning also stops on ♡, on a manual station choice, on pause, and on any error that empties the batch.
* The history still records titles announced during a scan: a discovery heard in passing is not lost (05 §3). Titles
  are marked as heard while scanning, so the History tab can fold them into one line per scan.
* Not counted as listening: a scanned station opens a listening session only once it is kept, or played for more than
  60 s. Otherwise "likes per hour" (05 §3) would be skewed by dozens of 15-second sessions.
* Not reported to Radio-Browser either (05 §4, click count), under the same rule: kept or 60 s. Reporting each step
  would inflate the popularity of whatever scanning happened to pass through.

No prefetching of the next stream: measured start is about half a second (05 §1), short enough on a 15 s turn, and
prefetching would open two connections per step for nothing.

### 4.3 Elsewhere (later)
"Take me somewhere": a random country (weighted by the number of working stations, so a country with three stations
does not come up as often as France), then a Surprise me draw in it, showing the country's name as it starts. The
cheap version of Radio Garden's random balloon ride, without a map (70 % of stations have no position, 08 §3).

---

## 5. Collections

### 5.1 Rule collections (first)
A collection is a **named query**, run live, never a stored list of stations: it cannot go stale when stations die.

* Definition: translated name + filters of 08 (country, genre, language, quality) + an order + an optional client-side
  check, defined in the client next to the genre list (08 §3.1), versioned with the translations.
* Order: random for genre and country collections (so the row changes at each visit and the same networks do not
  always sit first), Trending (`clicktrend`) only where it is the point.
* Each row shows ~10 stations, with Surprise me inside the collection (§4.1 with the collection's filters) and "See
  all", which opens the search results with the collection's chips set.

First set, in this order on screen:

| Collection | Rule |
|---|---|
| **From your country** | `countrycode` from the system locale, random order. |
| **In your language** | `language` from the system locale, outside your country. |
| **Stations that played artists you like** | Personal, §6.2. Hidden until there are enough likes. |
| **Your hidden gems** | Personal, §5.3. Hidden until there is enough listening. |
| One row per main genre family | Jazz, Classical, Electronic, Ambient, Lo-fi, World… (08 §3.1), `bitrateMin=128`. |
| **Trending now** | `clicktrend`, `hidebroken=true`. The only popularity row, last. |

Rows are fetched as they scroll into view, not all at the opening of the screen.

### 5.2 Curated lists
"French community radios" or "Ambient for focus" cannot be expressed as catalog filters: they need stations chosen
by people. Specified in [15-curated-lists.md](15-curated-lists.md): lists live in `getsoundfolk/soundfolk-stations`,
are fetched once a day, and each one is a row here, filtered by the same chips (14 §7, option D). "See all" opens the
list's screen, in the curator's order. The *No commercial ads* badge is a station attribute, not a list (14 §5); its
row is a rule collection over that attribute.

### 5.3 Hidden gems
The vision promises a community that "digs for hidden gems" (01). Before there is a community, the app can find the
listener's own:

* **Your hidden gems:** stations with a high like rate (likes per hour, 05 §3) and a low global `clickcount`, among
  those listened to for at least an hour in total. Local data only.
* **Catalog gems** (once the server exists): stations with few clicks but good station profiles across folkers
  (diversity, little repetition, few titles without an artist: 05 §3), kept and liked by several of them. Needs
  shared, opt-in data: not before a privacy design of its own.

---

## 6. More like this

### 6.1 From the catalog
In the station sheet, and as the fallback when a stream fails and the station has no other stream (the
fallback to another stream of the same station stays in [TODO.md](TODO.md)).

* Take the station's tags, mapped to genre families where possible (08 §3.1); keep the **most specific** one or two
  (fewest stations in `/json/tags`): `bossa nova` says more than `music`.
* One request per kept tag, `hidebroken=true`, same language first, then same country, then the rest.
* Rank client-side by tag overlap (shared tags ÷ all tags of both), same quality floor as §4.1.
* A station without a tag gets no "More like this" rather than a random list pretending to be similar.

### 6.2 From the listener's ears
The history already knows which artists each station plays (stats: "tracks heard on several stations").

* **Stations that played artists you like:** stations heard in the history (any station, not only favorites) that
  announced artists the listener liked elsewhere, ranked by the number of such artists, excluding favorites.
* Limited to stations already heard: the app does not know what unheard stations play. Reaching unheard stations
  this way needs a shared index of station → artists (§5.3, server side), not a local guess.
* Uses parsed artists, and MusicBrainz artist IDs where tracks were identified (05 §4), so `The Beatles` and
  `Beatles, The` count once.

---

## 7. Not done, and why

* **3D globe / map:** heavy, and 70 % of stations have no position (08 §3). Elsewhere (§4.3) gives the travel without
  the map.
* **Recently added:** the catalog has no creation date (§2). Sorting by `changetimestamp` would show recently *edited*
  stations under a false name. If wanted, the server can record when it first sees a UUID.
* **Skip talk** (skip speech and ads automatically): speech detection in the audio engine, on every stream, all the
  time — CPU and battery for a guess that fails on talk-heavy music shows. To reconsider only as an option of Scan.
* **Most voted / most clicked as home rows:** 06 §5. One Trending row at the end is the exception.

---

## 8. Changes

**Backend** (`catalog/`, `db/`)
- [ ] `surprise_station(filters, exclude)` per §4.1: random batch, client-side floor, exclusions (heard in 30 days,
  favorites, drawn this session), survivors ordered; returns the batch so "Another one" needs no new request.
- [ ] Recently heard station IDs and favorites as an exclusion set (from `listening_sessions` and favorites).
- [ ] Listening session and click report deferred until a station is kept or played for 60 s, when it was started by
  Scan.
- [ ] Mark history entries heard while scanning.
- [ ] `similar_stations(station_id)` per §6.1 (`/json/tags` counts cached for the session).
- [ ] `stations_by_liked_artists()` and `hidden_gems()` per §6.2 and §5.3, from the local database.
- [ ] Tests: floor and exclusions on a fixed batch, tag specificity and overlap ranking, deferred session rule.

**Frontend**
- [ ] Discover tab (06 §1): Surprise me + chips, Scan, collection rows (curated lists included, 14 §7), search box.
- [ ] Collection definitions next to the genre list, translated.
- [ ] Scan bar and its state machine (step, keep, next, stop back to the previous station).
- [ ] "Another one" in the player after a surprise, on media "next" too.
- [ ] "More like this" in the station sheet.
- [ ] When a stream fails and the station has no other stream: offer (never start silently) the first station of
  "More like this", in the player's error message.
- [ ] History: one folded line per scan.

**Later**
- [ ] Elsewhere (§4.3).
- [ ] Curated lists and the *No commercial ads* badge: see [15-curated-lists.md](15-curated-lists.md) §8.
- [ ] Catalog gems and the shared station → artists index (§5.3, §6.2).

---

## 9. Open questions

* Is "not heard in 30 days" the right exclusion? It keeps surprises new, but an explorer may want to stumble again on
  a station they forgot to add to favorites. Alternative: exclude only stations heard for more than a few minutes.
* 64 kbit/s as the floor, 128 for genre rows: same question as 08 §8 about codecs.
* Scan interval: is 15 s enough to judge a station that is between two tracks or in an ad break?
* Should Discover replace the Search tab now (three destinations of 06 §1), or come as a fourth tab until Listen
  exists?
