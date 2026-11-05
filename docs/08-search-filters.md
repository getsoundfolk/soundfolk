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
(see §4, empty results).

---

## 3. Filters offered

Chips above the results, applied at once, combinable, typed text narrowing what the chips selected (06 §4). No
separate filter screen.

| Chip | Values | Sent as | Default |
|---|---|---|---|
| **Country** | Countries from `/json/countries`, localised names, sorted by name; the system locale's country first. | `countrycode` | none |
| **Genre** | A curated list of ~30 translated genres (§3.1), not raw tags. | `tag` (substring) | none |
| **Language** | Languages from `/json/languages` with more than ~20 stations, localised names; the system language first. | `language` (English name) | none |
| **Good quality** | A switch: ≥ 128 kbit/s. An *HD* badge marks ≥ 192 kbit/s or lossless in rows (06 §5). | `bitrateMin=128` | off |
| **Working now** | A switch. | `hidebroken=true` | off (§3.2) |
| **Sort** | Relevance, Popular, Trending, Most voted, Name, Surprise me. | `order` | Relevance (§3.3) |

**Not offered as chips:**
* **Codec**: the desktop engine plays every codec the catalog lists except HLS, and the listener does not choose a
  station by codec. Shown in the station sheet only. To reconsider on mobile if a platform player fails on one.
* **HTTPS only**: the engine plays HTTP streams; nothing on desktop depends on it. The station sheet says when a
  stream is not encrypted.
* **Region, map, near me**: 70 % of stations have no position, so a "near me" search (`geo_lat` / `geo_long` /
  `geo_distance`) would miss most local stations. Later, in Discover, not in search; the country filter answers
  "stations from my country" today.

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

### 3.2 Working now
Off by default, as today: the catalog check gives false negatives (brief outages, geo-blocking of the checking
servers), so suspect stations stay visible with a warning and are ranked after healthy ones (`Health::Unreachable`).
The switch is for the listener who wants none of them. If listeners turn it on most of the time, make it the default.

### 3.3 Sort
06 §5: popularity is never the default, it puts the largest commercial networks first.
* **Relevance** (default): with text, names that start with the text, then names that contain it as a word, then the
  rest, each group by click count. Without text, click count (the catalog's only proxy for "good stations" until
  curated collections exist). Computed client-side on what the catalog returned.
* **Popular** `clickcount`, **Trending** `clicktrend`, **Most voted** `votes`, **Name** `name` (ascending),
  all descending except Name.
* **Surprise me** `order=random` with the active filters: one tap plays a random working station that matches (with
  `hidebroken=true` whatever the switch says). Not "completely random": the filters make it a surprise *within* what
  the listener chose.

Healthy stations stay first in every order except Name.

---

## 4. Behaviour

* **Pagination** is unchanged: the offset counts catalog entries, not shown stations (client-side drops: HLS,
  duplicates). Filters are server-side, so they do not make pages emptier; only the genre OR (§3.1) would.
* **Changing a chip** starts a new search (offset 0), with the same debounce as typing.
* **Empty results** with filters: say which filter excludes most ("No station in French with Good quality — 12
  without the quality filter"), with a button removing it. One extra count request per empty result, not more.
* **Fields the catalog leaves empty** are never shown as "Unknown" (06 §7); a filter that hides them says so once in
  its picker ("Stations without a language are not shown").
* **Persistence**: the chips last for the session (until the app closes). No saved filters (06 §4: no "filter feeds"
  to set up first). If a pinned search is wanted later, it belongs in Discover as a collection, not in search.
* **Lists** (`countries`, `languages`): fetched at the first opening of a picker, kept in memory for the session.
  They change slowly; a daily SQLite cache is enough if startup or offline use needs it.

---

## 5. Text that understands filters

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

## 6. Changes

**Backend** (`catalog/`)
- [ ] `SearchQuery`: add `language`, `bitrate_min`, `working_only`, `order`; sanitise them like the existing fields
  (`language` lower case, `bitrate_min` ≤ 320, `order` from a closed enum).
- [ ] `search_params`: send them; `hidebroken` follows `working_only`; `order`/`reverse` follow the sort. Always
  send `order`: the catalog's default is `name`, and an unknown value silently falls back to it.
- [ ] Relevance ranking client-side when the sort is Relevance and there is text.
- [ ] Commands `list_countries`, `list_languages` (`/json/countries`, `/json/languages`, in-memory cache).
- [ ] `surprise_station(filters)`: `order=random`, `limit` a few, first playable one.
- [ ] Tests: parameters per filter combination, relevance order, sanitising.

**Frontend** (`StationBrowser.svelte`, `api.ts`)
- [ ] Chip bar under the search box: Country, Genre, Language, Good quality, Working now, Sort; each picker searchable.
- [ ] Genre list with translations.
- [ ] Empty-result message naming the most restrictive filter.
- [ ] Surprise me button.

**Later**
- [ ] Text that understands filters (§5).
- [ ] Multi-genre OR (§3.1).
- [ ] Default country and language from the system locale, as preselected chips the listener can remove.

---

## 7. Open questions

* Should the system country be preselected? It answers "stations from my country" at once, but hides the rest of the
  world from someone looking for a station abroad by name. Proposal: preselect only when the text box is empty
  (browsing), never while a name is typed.
* `language` matches the English names the catalog stores (`french`, `german`); a few entries hold local names
  (`deutsch`, `español`). Send both names for the main languages, or accept the loss?
* Is ≥ 128 kbit/s the right single threshold? AAC+ at 64 kbit/s sounds better than MP3 at 128. A codec-aware
  threshold (AAC/AAC+/Ogg ≥ 64, MP3 ≥ 128) cannot be sent to the catalog in one request; it would be a client-side
  filter, with emptier pages.
