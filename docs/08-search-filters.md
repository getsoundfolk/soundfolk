# 🔎 Search Filters

Spec for filtering catalog search: what listeners ask for, what Radio-Browser can answer, and how it fits the UI rules
of [06-ui-principles.md](06-ui-principles.md) (§4 "Filters on the results", §5 "Human information first").

**Today:** the Search tab sends only the typed text. The backend already accepts `country_code` and `tag`
(`SearchQuery`, `catalog/mod.rs`) but nothing in the UI sets them. Results are ordered by click count, healthy
stations first; HLS stations and duplicates are dropped client-side.

---

## 1. What listeners ask for

From a synthesis of feedback on radio players (Reddit, GitHub, F-Droid forums, about radio-browser.info, Transistor,
Radio Garden, TuneIn, Shortwave…). The synthesis cites no sources: treat it as a ranking of hypotheses, not data.

| Request | Weight | Notes |
|---|---|---|
| Combine filters ("Jazz + France + ≥128 kbit/s") | very high | The most cited gap in other players. |
| Country | very high | "Stations from my country", or from one country. |
| Genre | very high | Radio-Browser tags are messy: a clean list is expected. |
| Language | high | Mostly for international stations. |
| Minimum quality | high | Avoid 32–64 kbit/s streams. |
| Hide dead stations | high | The long tail of the catalog is full of them. |
| Sort (popularity, trend, name…) | high | |
| Text that understands "jazz france" | high | Country and genre recognised in the search box. |
| Codec | medium | Mostly devices that fail on some codecs (Chromecast, old phones). |
| HTTPS only | medium | |
| Region / state, map | medium | Large countries; discovery à la Radio Garden. |
| Saved filters, quick chips, "local stations" | frequent | |

---

## 2. What the catalog can answer

`/json/stations/search` parameters, tested on `de1.api.radio-browser.info` on 2026-11-05 and read in the server
source (`radiobrowser-api-rust` at `454e08e`, 2026-10-25: parameters in `src/api/mod.rs`, SQL in
`get_stations_advanced`, `src/db/db_mysql/mod.rs`). Coverage measured on the 3,000 most-clicked stations
(`hidebroken=false`).

All filters are joined with AND in one SQL query. Text filters use MySQL `LIKE` on the stored column, so they follow the
column's collation (case-insensitive) and `%` / `_` in the value are wildcards (§3.1).

| Filter | Parameter | Behaviour | Coverage |
|---|---|---|---|
| Country | `countrycode` | ISO 3166-1 alpha-2, exact, case-insensitive. | 100 % have one |
| Genre | `tag` | `Tags LIKE '%tag%'` on the whole comma-separated tag string: `jazz` also finds `acid jazz`, `smooth jazz` (500 results, 393 with the exact tag). Also catches unwanted words: `rap` finds `trap`. | 24 % have no tag |
| Genre, exact | `tag` + `tagExact=true` | One whole tag of the list equals the value. | |
| Several genres | `tagList=a,b` | **AND**: every tag must be present (`jazz,lounge` → 123 stations, all with both); substring or exact per `tagExact`. No OR. | |
| Language | `language` | `Language LIKE '%value%'` on the English names (`french` matches `english,french`). `languageExact=true`: one whole item of the list. | 24 % have none |
| Language code | `languagecodes` | **Ignored**: parsed into the request parameters but never passed to the search query (and `languagecodes=fr` returns unfiltered results). Do not use. | |
| Minimum bitrate | `bitrateMin` | `Bitrate >= value` (with `bitrateMax`, default 1,000,000). Stations with bitrate `0` (unknown) are excluded. | 16 % are `0` |
| Codec | `codec` | Exact, case-insensitive (`MP3`, `AAC`, `AAC+`, `OGG`…). Top 3,000: MP3 68 %, AAC/AAC+ 26 %, unknown 5 %, Ogg 1 %. | |
| HTTPS only | `is_https=true` | Tests the **resolved** stream URL (`url_resolved`), not the submitted one. `false` keeps only HTTP. | 66 % are HTTPS |
| Working now | `hidebroken=true` | `LastCheckOK=TRUE`. | 0 % failing in the top 3,000: matters for the long tail |
| Has a position | `has_geo_info=true` | Latitude and longitude both set. | 70 % have none |
| Near a point | `geo_lat`, `geo_long`, `geo_distance` (metres) | Stations within the distance (`ST_Distance_Sphere`), only those with a position. | 30 % have one |
| Sort | `order` + `reverse` | Closed list: `name`, `url`, `homepage`, `favicon`, `tags`, `country`, `state`, `language`, `votes`, `codec`, `bitrate`, `lastcheckok`, `lastchecktime`, `clicktimestamp`, `clickcount`, `clicktrend`, `changetimestamp`, `random`. An unknown value silently falls back to `name`; the default is `name` ascending. | |

Lists for the pickers, each with a `stationcount`: `/json/countries` (name + ISO code), `/json/languages` (name +
ISO 639 code), `/json/tags`, `/json/codecs`. Top tags are noise as much as genres (`pop`, `music`, `news`, `rock`,
`radio`…).

Every filter on a field the catalog often leaves empty also hides the stations that never filled it: a language filter
hides a quarter of the popular stations, a quality filter a sixth. The UI must not pretend these stations do not exist
(see §5, empty results).

---

## 3. Filters offered

Search stays one text box. Under it, **three chips**, for what a listener says ("some jazz", "a Belgian station",
"in Spanish"): applied at once, combinable, typed text narrowing what the chips selected (06 §4). No separate filter
screen, no "Apply" button, no count of active filters.

| Chip | Values | Sent as |
|---|---|---|
| **Genre** | A curated list of ~30 translated genres (§3.1), not raw tags. | `tag` (substring) |
| **Country** | Countries from `/json/countries`, localised names, sorted by name; the system locale's country first. | `countrycode` |
| **Language** | Languages from `/json/languages` with more than ~20 stations, localised names; the system language first. | `language` (English name) |

* An empty chip shows its name ("Country"); a set one shows its value with a remove button ("Belgium ✕").
* Each picker has a search field.
* The text box can set the chips itself (§6): most listeners never open a picker.

Everything else a filter would do is a **default**, so that nobody has to set it:
* **Quality** (§3.2): of a station's copies, the highest bitrate is shown; an *HD* badge in the row.
* **Working stations** (§3.3): first in the list; suspect ones stay, after them, with a warning.
* **Order** (§3.4): relevance only. Popularity, trends and chance belong to discovery
  ([09-discovery.md](09-discovery.md): Surprise me, Trending row), not to search.

**Not offered as chips:**
* **Codec**: the desktop engine plays every codec the catalog lists except HLS, and the listener does not choose a
  station by codec. Shown in the station sheet only. To reconsider on mobile if a platform player fails on one.
* **HTTPS only**: the engine plays HTTP streams; nothing on desktop depends on it. The station sheet says when a
  stream is not encrypted.
* **Region, map, near me**: 70 % of stations have no position, so a "near me" search (`geo_lat` / `geo_long` /
  `geo_distance`) would miss most local stations. Later, in Discover ([09-discovery.md](09-discovery.md)), not in
  search; the country filter answers "stations from my country" today.

### 3.1 Genres
One genre = one translated label + one catalog tag sent as a substring:
`Jazz → jazz`, `Classical → classical`, `Electronic → electronic`, `Ambient → ambient`, `Lo-fi → lofi`,
`News → news`, `Talk → talk`, `Hip-hop → hip hop`…

The substring is what a listener means by a genre family (`jazz` brings `smooth jazz`, `latin jazz`). Short tags catch
unrelated words (`rap` finds `trap`): use `tagExact=true` for those. Spelling variants are covered by the `LIKE`
wildcards, as the name search already does (`NameQuery`, with the same "does `%` still work" check): `hip%hop` finds
`hip hop`, `hip-hop` and `hiphop` in one request. Variants with nothing in common (`hip hop` / `rap`) need one request
per tag, then merge; only for the few genres that need it. The list lives in the client, versioned with the
translations; it is not fetched.

Two genres at once mean **OR** for a listener ("jazz or soul"), but `tagList` is an AND. First version: **one genre
at a time**. Multi-genre OR, if asked for, is one request per genre then merge, deduplicate and re-sort client-side.

### 3.2 Quality
No `bitrateMin` filter: it would also hide the 16 % of stations whose bitrate is unknown. Instead:
* **Duplicates** (`catalog/dedup.rs`): of the copies of one station, the one shown is the working one, then the highest
  bitrate (unknown counts as the lowest), then the most clicked. It takes the place of the group's first copy in the
  list. A favorite copy always stays, whatever its bitrate.
* **No other ranking** of copies: clicks are not summed over a group, and the station sheet lists the other copies
  without a "most used" label or a particular order.
* The *HD* badge (§4) says which stations stream at a high bitrate.

### 3.3 Working stations
No "Working now" switch: the catalog check gives false negatives (brief outages, geo-blocking of the checking
servers), so suspect stations stay visible, ranked after healthy ones (`Health::Unreachable`), with a short warning
(§4). If listeners still ask to hide them, a switch can come back.

### 3.4 Order
06 §5: popularity is never the default, it puts the largest commercial networks first. Search has one order,
**relevance**: with text, names that start with the text, then names that contain it as a word, then the rest, each
group by click count. Without text, click count (the catalog's only proxy for "good stations" until curated
collections exist). Computed client-side on what the catalog returned; healthy stations stay first.

---

## 4. Result rows

What a row shows decides what the filters need to do. Today's row (`FR · MP3 128 kbit/s · pop, music`) breaks 06 §5:
codec, bitrate and raw tags, no logo. Measured on the 3,000 most-clicked stations: 72 % have a logo, 76 % at least
one tag; 5 % of the names start or end with spaces or a tab, 11 % are in capitals, 5 % are longer than 40 characters.

```
[logo]  Station name                                   [HD]  ☆  ⓘ
        Jazz · Lounge · Belgium
        ⚠ Maybe offline                 ← only when the last check failed
```

* **Logo**: the favicon, loaded when the row scrolls into view, cached. Missing or failing: a tile with the name's
  initial, its colour drawn from the name. Never a broken image.
* **Name**: cleaned for display (§4.1), one line, cut with "…", the full name in a tooltip.
* **Second line**: up to two translated genres (§3.1; tags that map to no genre are not shown, `music`, `radio` or
  `hits` never), then the country in words. What an active chip already filters is not repeated: with [Belgium] set,
  no row says "Belgium". The language is not shown, even when it differs from the country's (an English station in
  France): it is in the station sheet.
* **Badges**: *HD* (≥ 192 kbit/s, or lossless) and *personal* only. *Ad-free* and *listener-supported* wait for a source
  (09 §5.2).
* **Warning**: "Maybe offline", short; the date of the last successful check in a tooltip.
* **Actions**: a click on the row plays (the main action); ☆ adds to favorites; ⓘ or a right-click (a long press on
  mobile) opens the station sheet (§4.2). The playing station is highlighted, a small animation in place of ▶.
* **Not in rows**: codec, bitrate in figures, language, votes, clicks, URL, raw tags.
* **Same row everywhere**: search, favorites, Discover and My stations (06 §8).
* **No highlight of the searched text** for now. The name search folds accents and separators (`NameQuery`), so a
  JavaScript `indexOf` would highlight some results and not others, with no visible reason. It comes when
  `NameQuery::matches` returns where the fragments are in the displayed name.

### 4.1 Station names
Display only: the raw name stays as is in the database, favorites and history, so the rule can change without losing
anything (as raw ICY titles, see the TODO).
* Remove invisible characters (direction marks, zero-width spaces: `ایران‎  Radio Liberty`) and collapse runs of
  spaces.
* At both ends, remove only `# * - = + ~ | < > _` and spaces: `# TOP 100…`, `* Meme dans les orties *`,
  `---تراتيل---`, `=KECO=70s ROCK`, `+Exclusively Bad Bunny`, `Grunge FM<`. Nothing else, as other characters are
  part of names: `.977 Hitz`, `Mouv'`, `Slow Rock 90'`, `NPO Radio %`, `CCTV-13 新闻（伴音）`.
* Keep emojis: rare (4 names in 3,000), sometimes part of the name (`🍒 La Radio sous la Cerise 🍒`,
  `Mega St☆r Spain`).
* Never change the case, never cut at the first " - " (12 % hold one, and `Radio Nacional de España - Radio 5` is a
  name).
* If nothing is left, show the raw name. Names broken in the catalog (`����� ����`) cannot be repaired.

### 4.2 Station sheet
Details are not shown on hover: hover does not exist on touch screens, is not reachable with the keyboard, and its
content can be neither clicked nor copied. Hover only gives the full name and the date of the last check.

The station sheet (`StationSheet.svelte`) shows the station in the player only, today; it opens for any station. Its
parts tied to playback (track on air, recipes) show only for the station playing. From a list, in this order:
1. Name, logo, genres, country, language, link to the website.
2. Quality in words, figures below: "Good quality — AAC 128 kbit/s".
3. **Other streams of this station** (its duplicates), each with its bitrate and ▶. This is where codec and bitrate
   help: choosing between two streams of the same station.
4. Folded, "Technical details": stream URL, raw tags, votes, clicks, last check.

The listen count goes to the entry actually played (its UUID), not to the one the row showed.

---

## 5. Behaviour

* **Pagination** is unchanged: the offset counts catalog entries, not shown stations (client-side drops: HLS,
  duplicates). Filters are server-side, so they do not make pages emptier; only the genre OR (§3.1) would.
* **Changing a chip** starts a new search (offset 0), with the same debounce as typing.
* **Empty results** with filters: one sentence and one button, naming the filter that excludes most ("No jazz
  station in Belgium. [Remove Belgium] (14 stations)"). One extra count request per empty result, not more.
* **Fields the catalog leaves empty** are never shown as "Unknown" (06 §7); a filter that hides them says so once in
  its picker ("Stations without a language are not shown").
* **Persistence**: the chips last for the session (until the app closes). No saved filters (06 §4: no "filter feeds"
  to set up first). If a pinned search is wanted later, it belongs in Discover as a collection, not in search.
* **Lists** (`countries`, `languages`): fetched at the first opening of a picker, kept in memory for the session.
  They change slowly; a daily SQLite cache is enough if startup or offline use needs it.

---

## 6. Text that understands filters

Typing `jazz france` should give French jazz stations, not stations named "jazz france".

* Tokens of the text are matched, case- and accent-insensitive, against **genre labels** (§3.1, every translation),
  **country names** (localised and English, plus ISO codes only when typed in upper case) and **language names**.
* **The whole text is filter words** (`jazz france`, `radio française`… minus a few stop words like `radio`,
  `stations`, `music`): the chips are set, the text box is emptied. The chips appear, so the listener sees what
  happened and removes one with a tap.
* **Part of the text is a filter word** (`france inter`, `jazz radio`, `classic fm`): this is most often a station
  name. Search the whole text as a name, as today, and offer the filter as a suggestion chip above the results
  ("Filter: France?"). Never apply it silently.
* Ambiguous words go to the station name: `classic` (genre or "Classic FM"), `deutsch`, country names that are also
  words.

Matching happens in Rust, next to `NameQuery`, so that the folding rules (`catalog_text`, `fold`) are shared.

---

## 7. Changes

**Backend** (`catalog/`)
- [x] Duplicates: the working copy, then the highest bitrate, then the most clicked (§3.2).
- [ ] `SearchQuery`: add `language`; sanitise it like the existing fields (lower case). Keep `country_code` and `tag`.
- [ ] `search_params`: send it. Always send `order`: the catalog's default is `name`, and an unknown value silently
  falls back to it.
- [ ] Relevance ranking client-side when there is text (§3.4).
- [ ] Commands `list_countries`, `list_languages` (`/json/countries`, `/json/languages`, in-memory cache).
- [ ] The other streams of a station, for the sheet (§4.2): the duplicates dropped from a group, kept with the
  station shown.
- [ ] Station name cleaning (§4.1), with tests on the names quoted there.
- [ ] Tests: parameters per filter combination, relevance order, sanitising.

**Frontend** (`StationBrowser.svelte`, `StationRow.svelte`, `StationSheet.svelte`, `api.ts`)
- [ ] Chip bar under the search box: Genre, Country, Language; each picker searchable.
- [ ] Genre list with translations.
- [ ] Result row (§4): logo or initial tile, cleaned name, genres and country, *HD* badge, short warning, row click
  plays, ⓘ and right-click open the sheet.
- [ ] Station sheet for any station, with the other streams (§4.2).
- [ ] Empty-result message naming the most restrictive filter.

**Later**
- [ ] Text that understands filters (§6).
- [ ] Highlight of the searched text, once `NameQuery` gives positions (§4).
- [ ] Multi-genre OR (§3.1).
- [ ] Default country and language from the system locale, as preselected chips the listener can remove.

---

## 8. Open questions

* Should the system country be preselected? It answers "stations from my country" at once, but hides the rest of the
  world from someone looking for a station abroad by name. Proposal: preselect only when the text box is empty
  (browsing), never while a name is typed.
* `language` matches the English names the catalog stores (`french`, `german`); a few entries hold local names
  (`deutsch`, `español`). Send both names for the main languages, or accept the loss?
* Is the raw bitrate the right measure of quality, for the *HD* badge and for picking a station's copy? AAC+ at
  64 kbit/s sounds better than MP3 at 128. A codec-aware measure (AAC/AAC+/Ogg counted double) is a client-side rule,
  easy to add if the copies picked sound worse.
