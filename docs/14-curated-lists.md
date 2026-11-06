# 📋 Curated lists

Lists of stations chosen by people ("Late-night jazz", "French community radios", "Ambient for coding"), as opposed to
the rule collections of [09-discovery.md](09-discovery.md) §5.1, which are live catalog queries. This is the
spec of 09 §5.2, with the ad-free attribute it depends on. Not built: §1–3 challenge a first proposal, §4 records
the decisions, §5–8 are the plan, §9 what is left to decide.

**Today:** nothing built. Before this document, 09 §5.2 put hand-picked collections on `soundfolk.fm`, fetched
daily, and left "who curates" open; it now points here. [12-donations.md](12-donations.md) built the first piece of
shared station data (`getsoundfolk/soundfolk-stations`, donation links only) and its whole chain: one file per
station, proof, bot, daily CI check, `v1/stations.json` fetched once a day with `If-None-Match`.

---

## 1. The proposal, in short

A brainstorm (pasted from a chat assistant, 2026-11-06) proposed:

1. Hand-picked lists as the "cleanest" answer to Radio-Browser's noise: dead streams, prerolls, bad tags.
2. A JSON list format (M3U only for import/export), one JSON per list plus a `manifest.json` with a `version` per
   list, in a public Git repository served through a CDN (jsDelivr / GitHub Pages).
3. Lists copied into SQLite (`curated_lists`, `curated_list_stations` keyed by Radio-Browser UUID), upserted when
   a list's version goes up.
4. **"In these lists"** on the station being played: the lists that contain it, one tap to open each.
5. Personal lists, same schema, exportable as M3U/JSON and shareable by link or short code.
6. Community features to make Soundfolk the player people use rather than VLC/mpv: lists published by users,
   "Community Verified" badge from peer validation, a "N folkers are listening now" counter, a community radio
   roulette, an "Listen on Soundfolk" button for station websites, notes on station sheets.

---

## 2. What holds

* **The need is real**, and it is the one 13-music-only §1 found: people ask for *stations* (SomaFM, Radio Paradise,
  FIP, Folk Alley…), not for a feature. A list answers that directly.
* **Lists are data, not code**, fetched daily, as 09 §5.2 already decided. A list shipped with a release goes stale.
* **"In these lists" on the station being played** is the best idea of the proposal: it turns a single listen into
  discovery by people, not by an algorithm, at the cost of a local lookup. It fits 06 (one level, never empty) as long
  as it is hidden when there is no list.
* **A public Git repository** as the source, with an open licence, is how the recipes and the donation links already
  work. Contributors already know the path.

---

## 3. What does not hold, or needs proof

### 3.1 The claims
* **"Absolute ad-free guarantee"** cannot be promised. A list checked by hand is right on the day it is checked;
  a station can start running ads (or be bought) the next week. 11 §2.6 already decided that URL signals can only
  *remove* a station from *Ad-free*, never add one, and 11 §3.5 that "I heard an ad" overrides any badge locally.
  The wording must say "picked by people as ad-free", with a date, not "guaranteed".
* **Several examples are wrong or unchecked:**
  * Radio Nova is a commercial network with ads, not a category A community station.
  * Category A community radios (FSER) *may* air ads, up to 20 % of their revenue. "Community" ≠ "ad-free".
  * Radio France: FIP has no commercial ads, but France Inter does, and France Culture / France Musique air
    sponsorship and public-interest spots. "Public service = no ads" is false as a rule.
  * KEXP, WXPN and NPR stations air **underwriting** (sponsor mentions) and fund drives. Whether that counts as an
    ad is a definition to choose (§6, Q4).
* **"Reddit and X love it", "immediate visibility"**: no source. Our own check (13 §1) could not reach Reddit or X.
  The social signal is still unmeasured; nothing in this document relies on it.

### 3.2 The data model
* **A list keyed only by Radio-Browser UUID breaks.** UUIDs disappear when an entry is deleted and re-created, and
  personal stations have none. The recipes and soundfolk-stations already use `match` (`stations` UUIDs +
  `streams` host/path): a list item should reuse it, or point at a soundfolk-stations entry. (Settled otherwise
  in §6.1: a UUID alone, with the daily check catching the ones that disappear.)
* **A `version` per list plus a `manifest.json`** duplicates what HTTP already gives: one file, `ETag` /
  `If-None-Match`, once a day, like `v1/stations.json` (12 §2.4). Per-list versions add a sync protocol for a file
  that will weigh a few tens of kB.
* **Copying the lists into SQLite** is not needed for speed: a few hundred entries in memory answer "which lists
  contain this station" instantly. The donation list is a cached file, not tables; doing the same keeps one pattern.
  SQLite is the place for **personal** lists, which are the listener's data.
* **"Ad-free" as a list** mixes two things: an *attribute* of a station (ad-free, listener-supported: 06 §5 badges,
  the *Ad-free* filter chip of 08) and an *editorial theme* (jazz, ambient, French community). If "Ad-free" is a
  list, the filter chip and the badge have to read a list; if it is an attribute in soundfolk-stations (12 §2.1
  already reserves `ad_free` "with its own proof"), every list, search and Surprise me can use it. Proposal: ad-free
  is an attribute, lists are themes.

### 3.3 The community features
* **"N folkers are listening now"** needs every client to tell a server what it plays, live. That is telemetry, and
  05 §4 says "no telemetry beyond" the Radio-Browser click count. It also needs the server that does not exist yet.
  Out of scope for this feature.
* **"Community Verified" by peer validation, author handles (`@Alex_FR`), user-published lists** all need identity:
  Soundfolk has no accounts. Without accounts, the only identity available is the contributor's GitHub account in
  the repository, and the only validation is the maintainer's merge (12 §2.2, the user is the sole CODEOWNER).
* **Lock-in vs export.** The goal "people use Soundfolk, not VLC" is reached through what a playlist file cannot
  carry: the history, likes, now-playing from the web, ads shown, loudness, donation links, "in these lists".
  Refusing an M3U export would not keep anyone and the lists are open data anyway (ODbL). Export is cheap and earns
  trust; import of an M3U is "add station by URL" (05 §2) in a loop.
* **Short codes and share links** need something to resolve them: a `soundfolk://` URL scheme registered by the app,
  or a page on `soundfolk.fm`. Neither exists. A file (JSON) is the only share that works today.
* **"Listen on Soundfolk" button, station notes, community roulette**: separate features, each with its own
  moderation. Not part of this one. The roulette is Surprise me (09 §4.1) restricted to the lists' stations: a
  one-line addition once lists exist.

### 3.4 Maintenance, the real cost
A list is editorial work that never ends (13 §3 already set it aside for this reason). Each item can die, change
its stream, start running ads. The donation chain answers that for links with a bot and a daily check; lists need
the same, or they rot silently:

* daily CI check that each item still resolves in Radio-Browser (UUID or stream) and that its stream answers;
* a dead item is left out of the published file, not deleted from the source, and an issue opened after N days
  (same rules as 12 §2.3);
* in the app, an item that does not play is skipped like in search (warning, ranked last, 09 §5.2).

---

## 4. Decisions (2026-11-06)

Taken from answers to the questions of the first draft (a review relayed by the user, then four points settled by
the user). Each line says where it applies below.

| # | Decision |
|---|---|
| Repository | `getsoundfolk/soundfolk-stations` grows: `lists/` next to `stations/`. One fetch, one bot, one CI. |
| First goal | The ad-free **attribute** first (§5), then thematic lists (§6). |
| Ad-free | An attribute of a station entry with its proof, shown as a badge, a filter and a Discover row built from the attribute. Never a hand-edited list. |
| Definition | "No commercial ads" (§5.1). Sponsor mentions without price or call to buy, self-promotion and donation drives are accepted. |
| Proof | Station's own statement, or a maintainer check; plus the daily 11 §2.6 URL check, which can only remove. |
| Curation | The maintainer writes the first lists, then pull requests from anyone. The maintainer alone merges, as for donations. |
| Credit | Each list names the GitHub logins of its curators (1 to 3); the app shows them. |
| Format | TOML source written by hand, JSON published by CI; fields in §6.1 and §6.3. |
| List criteria | The curator's own. Ads do not matter in a list: a station with local sponsorship belongs in a list if the curator says so. The ad-free badge stays on stations (§5). |
| Size | 5 to 40 stations per list (the rule); 10 to 15 for the launch lists (the aim). |
| Launch lists | Four, written by the maintainer (§6.6): `fr-community-radios` first, then `ambient-focus`, `eclectic-discovery`, `public-cultural`. |
| List filters | Designed now, so the list format carries what they need (§6.5). |
| Navigation | Lists are Discover rows under the same chips as stations; personal lists (v2) in Listen (§7, option D). |
| Items | One Radio-Browser UUID per station, nothing else (§6.1). |
| In the app | "In these lists": chips on the station sheet; in the player, one discreet "In N lists" link, hidden when N = 0. |
| Order | The curator's order. |
| Title language | Title and description written once, in one language the curator picks for the list's listeners (French for French community radios, English for an international list), shown as is everywhere, like a playlist name. The vocabulary of `use` and `genres` is translated by the app. |
| Personal lists | v2, independent of favorites. |
| Sharing | No `soundfolk://` scheme. Lists are proposed by pull request or a prefilled issue. |
| M3U | Export offered (a published list in v1, personal lists in v2). |
| Item notes | Later, not in schema 1. |
| Review date | Never shown to listeners, no "outdated" warning (06 §6). Liveness is the daily CI's job; a yearly review issue and a 30-day rule for unanswered breakage stay on the repository side (§6.2). |
| Out of scope | Live listener counter, peer badges, "Listen on Soundfolk" button, public station notes. |
| Demand | Build now: the need for the usual ad-free stations is established (13 §1). |

---

## 5. Ad-free attribute

### 5.1 Definition
The badge reads **No commercial ads**, never "No ads". A station qualifies when its stream carries no spot sold to a
third party.

| Accepted | Disqualifies |
|---|---|
| Sponsor mentions without price or call to buy (US underwriting, French *parrainage*) | Spots sold to third parties, including public-service ones (France Inter) |
| Self-promotion, jingles, station shows | Server-side insertion by an ad platform (AdsWizz, Triton, 11 §2.1 and §2.6) |
| Donation drives (SomaFM, KEXP, Radio Paradise) | Prerolls on connect |

The station sheet links to this definition ("What does this mean?"), so a listener who hears a sponsor mention on
KEXP knows it was counted as allowed.

### 5.2 In soundfolk-stations
An entry gets an optional `[ad_free]` section, next to `[donate]`, each with its own proof (12 §2.1 announced it).
An entry may have either section or both.

```toml
[ad_free]
statement = "https://somafm.com/about/"   # page of the station's own website saying it airs no ads
# or, maintainer only, when the website says nothing:
# checked_by_hand = "2026-11-06: 3 h listened, sponsor mentions only"
```

* `statement` must be on the domain of `homepage`, like `donate.found_on`. The bot does not try to read the
  meaning of the page; the maintainer does, at review.
* `checked_by_hand`: maintainer only, refused by CI in anyone else's pull request (same rule as donations).
* **Daily CI:** a station whose stream URL (`url` or `url_resolved` in Radio-Browser) matches the 11 §2.6 list is
  left out of the published attribute at once and an issue is opened. The statement page is checked to still
  answer, not to still say the same thing.
* Published in `v1/stations.json` as `ad_free: true` (the proof stays in the repository). Unknown keys are ignored
  by today's clients (12 §2.4), so `v1` holds.
* The memory decision "schema 1 carries only `[donate]`" changes with this section.

### 5.3 In the app
* **Badge** "No commercial ads" on the row and the sheet (06 §5).
* **Ad-free chip** in search and Discover filters (08): the attribute, minus stations the listener marked with
  "I heard an ad" (11 §3.5), which always wins locally.
* **Discover row** "No commercial ads": a rule collection (09 §5.1) over the attribute, random order, Surprise me
  inside it. No size cap: it is not a list.

---

## 6. Thematic lists

### 6.1 Source
`lists/<id>.toml` in soundfolk-stations. TOML because people write and review these files by hand in pull requests:
comments, no trailing-comma errors, one short block per station in a diff. The app never reads TOML: CI publishes
JSON (§6.3), as for the donation links.

```toml
#:schema ../list.schema.json
schema = 1
title = "Radios associatives en France"
description = "Des radios locales tenues par des associations en France, des vallées aux villes."
curators = ["raphaelh"]
checked = 2026-11-06
use = ["discovery", "shows"]
genres = []

stations = [
  "<Radio-Browser UUID>",  # Fréquence 7
  "<Radio-Browser UUID>",  # Radio Campus Clermont-Ferrand
]
```

**List fields**

| Field | Required | Type | Rule | Used for |
|---|---|---|---|---|
| (file name) | yes | `[a-z0-9-]` | Stable: renaming a list is a new list | id, links |
| `schema` | yes | `1` | Another value is refused by the app | format version |
| `title` | yes | text | One language, picked by the curator for the list's listeners; 40 characters at most | row header, chips |
| `description` | yes | text | Same language as `title`; 140 characters at most, one sentence | row header, list screen, search |
| `curators` | yes | logins | 1 to 3 GitHub logins; the first answers CI issues | "by @login", search |
| `checked` | yes | date | Last full review by hand; never shown (06 §6) | yearly review (§6.2) |
| `use` | yes | ids | 1 to 3, from `vocab.toml` | Use chip |
| `genres` | yes | ids | 0 to 3, from `vocab.toml` (08 §3.1 families); empty = no dominant genre | Genre chip |

**Stations:** `stations`, an array of Radio-Browser UUIDs, 5 to 40, in the curator's order, no UUID twice. Nothing
else per station:
* **No name field.** A comment after each UUID says which station it is, for whoever reads the file. In a pull
  request, CI posts the Radio-Browser name of every UUID added or moved, so the reviewer sees at once a UUID pasted
  from the wrong station. A name field would only repeat that check, and drift from Radio-Browser's name.
  The app shows the Radio-Browser name as is, even an untidy one ("RADIO PRIMITIVE", "Fip Monde (metadata)"),
  the same name the station has everywhere else in the app.
* **No stream fallback.** A UUID that disappears from Radio-Browser (entry deleted, or re-created under a new UUID)
  is caught by the daily check (§6.2): the station is left out and the curators get an issue, and they paste the new
  UUID. How often that happens is not measured; a fallback can come if it turns out frequent.
* **No reference to a `stations/` entry.** It would couple the two kinds of files for no gain: an entry has a UUID
  too.
* **Duplicates of the same station** (Radio-Browser often lists one station several times, by stream or bitrate):
  the curator picks one UUID. CI publishes that entry's stream with it (§6.3), so the app also recognises a
  duplicate playing the same stream, without the curator listing it.

**Left out, and why**

| Proposed field | Why not |
|---|---|
| `id` inside the file | Duplicates the file name; the two would drift. |
| `version`, `created_at`, `updated_at` | Git history has them; the app syncs with the HTTP `ETag`, not per-list versions (§3.2); a date shown would be visible maintenance (06 §6). |
| `author` next to `maintainers` | One field, `curators`: the creator who leaves stops answering, so credit goes to whoever maintains it. |
| Free `tags` | Free tags are Radio-Browser's problem (09 §2): spelling variants, no translation. `use` and `genres` are a closed, translated vocabulary; text search covers the rest. |
| `country` written by hand | Computed by CI from the stations (§6.3): Radio-Browser's country code is nearly always there and nearly always right (§6.3). A `WW` code does not exist in ISO 3166. |
| `genres` computed | Radio-Browser's tags and languages are missing for a third of the stations (1,000 drawn at random, 2026-11-06: no tags 33 %, no language 35 %, no country code 1.5 %). The curator knows the list better. |
| Ad-free count, "100 % ad-free" flag | Lists ignore ads (§4); the badge is on each station (§5). |
| A note per station | Later (§4). It will need a per-station table instead of a plain array: a format change that the published JSON hides from the app (§6.3). |
| `name` per station, stream fallback, `stations/` reference | Above. |
| Titles in several languages | A list is an authored work, like a playlist name: written once, in the language its curator picks for its listeners, rather than an English title required from everyone. Optional translations can come later; unknown keys are ignored. |
| `region` | Title and description already name the place ("Ardèche"), and search reads them. |
| `languages` | It only matters for lists listened to for their speech, and every station would then have to speak the language, or the Language chip would mislead. At launch the one such list is French only, so Country and its French title already say it; the three others are music, where language does not matter. Country is not language (French in Belgium, Switzerland, Québec), so a `language` field, singular, with every station speaking it as its main language, can come with the first list that needs it; unknown keys are ignored. |
| Logo or cover image | No logos for now (soundfolk-stations scope). |

* `curators` must include the author of the pull request that creates the list, or the repository maintainer. A
  pull request changing a list whose curators do not include its author gets a warning in CI, for the maintainer
  to ask them.
* Licence: ODbL / DbCL, like the rest of the repository.

### 6.2 Checks
* `lint.py`: schema, sizes, UUID syntax, no UUID twice, `use` and `genres` from `vocab.toml`. In a pull request,
  CI writes in the job summary the Radio-Browser name, country and stream state of every station of the lists that
  changed (a comment would need write access, which pull requests from forks don't get).
* **Daily CI**, same rules as 12 §2.3: a UUID no longer in Radio-Browser is left out of the published list and an
  issue is opened at once. An item whose stream does not answer is left out (the source file does not change) and
  comes back when it answers again; an issue after 7 days. "Does not answer" means from CI and from Radio-Browser's
  own checkers (`lastcheckok`): a stream blocked outside its country fails from a US runner only, and stays in. A
  list that falls under 5 published items is published anyway, with an issue.
* The same "more than 20 % failing at once" guard as donations.
* **A stable list is not stale.** A list nobody edited for years is fine as long as the daily check passes: age
  alone never triggers anything in the app.
* **Unanswered breakage:** a list issue opened by CI and left without an answer from the list's curators for
  30 days goes to the repository maintainer, who fixes the list, takes it over (`curators` changes), or
  unpublishes it.
* **Yearly review:** once a year after `checked`, CI opens an issue for the list's curators, because the daily
  check cannot see everything (a station bought, a change of format, a show the list was built around that ended).
  Closing it means bumping `checked`, with no other change needed. Left unanswered, it changes nothing in the app.

### 6.3 Published
`v1/lists.json`, built by CI from `lists/` and `stations/`:

```json
{
  "version": 1,
  "lists": [
    {
      "id": "fr-community-radios",
      "title": "Radios associatives en France",
      "description": "…",
      "curators": ["raphaelh"],
      "use": ["discovery", "shows"],
      "genres": [],
      "countries": ["FR"],
      "items": [
        { "uuid": "<UUID>", "stream": "host/path" }
      ]
    }
  ]
}
```

* Lists sorted by id, for stable diffs; the app shows them by title, in alphabetical order with the listener's
  locale collation ("Écoute" next to "Ecology"). No order file: nobody ranks one curator's list above another's,
  and a new list needs no placing. Items in the curator's order, minus the ones left out by the daily check (§6.2).
* `stream`: host and path of the entry's `url_resolved`, taken from Radio-Browser by CI, never written by hand. The
  app matches a station by UUID, or by the same stream (duplicates, §6.1). `checked` is not published (no date in
  the app).
* `countries` is computed from the stations' Radio-Browser `countrycode` (ISO 3166-1 alpha-2), each country kept
  when it covers at least 20 % of the stations (one Belgian station does not make a French list Belgian). A list
  with no country above 20 % gets an empty array, and the chip skips it.
  Checked on 1,000 stations drawn at random (2026-11-06): 1.5 % have no country code. Of the 419 whose homepage has
  a country domain (`.fr`, `.de`, `.co.uk`…, vanity ones like `.fm` left out), 412 agree with it (98 %); of the 7
  others, one is plainly wrong (a UK station marked Afghanistan, the first country of an alphabetical list), the
  rest are real cases (Guadeloupe 1ère on `francetvinfo.fr`, a Belarus station on a `.ru` site, free hosting). Not
  checkable this way: stations on generic domains (`.com`, `.org`), and web radios with no country of their own.
  Genres are the curator's (§6.1): Radio-Browser lacks tags too often.
* Fetched once a day with `If-None-Match` through `daily_list.rs`, cached, held in memory, also when web titles are
  off (12 §2.4). `SOUNDFOLK_LISTS_URL` replaces the address. Unknown keys are ignored; another `version` is refused.
* The app adds what it can count itself: the number of stations, shown on the list's screen.

### 6.4 In the app
* `lists_for_station(station_id, url)`: the lists whose items match, in the published order of lists.
* **Station sheet:** "In these lists" chips (the list titles), hidden when there are none. A chip opens
  the list.
* **Player:** one discreet "In N lists" link that opens the station sheet on that section, hidden when N = 0. N
  counts thematic lists only, never the "No commercial ads" attribute (a badge, not a list). Never the list names in
  the player (06 §2: station, track, ♡, donation link).
* **A list's screen:** title, description, "by @login" (each curator), number of stations, stations in the curator's
  order (dead ones ranked last with a warning, 09 §5.2), Surprise me inside the list, **Export M3U**.
* **Where lists are browsed:** §7. Until the Discover tab of 09 exists, a **Lists** tab between Search and
  Favorites shows them, sorted by title, each opening its screen.
* **Suggest a list** (Discover, end of the rows): opens the repository's contributing page. A list is several
  stations and a text: a pull request, or an issue form for those without Git.

### 6.5 Filters on lists
Station filters (08) work on Radio-Browser fields. Lists carry fields that match them where possible (Genre,
Country), so one set of chips can filter both (§7 D), plus the fields only a list has. All optional,
combined with AND:

| Filter | Values | Where it comes from |
|---|---|---|
| **Use** | Focus, Calm, Energy, Discovery, Shows | `use`, written by the curator from `vocab.toml` |
| **Genre** | The genre families of 08 §3.1 (Jazz, Classical, Electronic, Ambient…) | `genres`, written by the curator |
| **Country** | Countries present in the lists | `countries`, computed by CI |
| Curator | Logins | `curators`, through the search box (§7 D) |
| Text | Title, description | Search box above the chips |

* `vocab.toml` in the repository holds the ids (the five uses below, and the genre ids of 08). The app translates
  them. A new id is a pull request, like a list.
* **Use** answers "what do you listen to this list for?", not "what is in the stream?":

  | id | fr | en | Listening for |
  |---|---|---|---|
  | `focus` | Focus | Focus | Work, study, code: few or no voices |
  | `calm` | Calme | Calm | Unwind, read, end of the day, sleep |
  | `energy` | Énergie | Energy | Sport, chores, commuting: a steady tempo |
  | `discovery` | Découverte | Discovery | Hearing what one does not know yet |
  | `shows` | Émissions | Shows | Hosts, programmes, local life, with music between them; not talk only |

  `shows` replaced "Talk" (a chip that read as talk radio only, when the main case is community radio mixing music
  and programmes), and took the place of "Culture" (mixed musical culture and speech) and "Everyday" (any station is
  listened to every day). It is not the station filter "Hide news & talk" of 13 §2.1, which works on Radio-Browser
  tags. The values come from how people describe listening, not from data (13 §1 could not reach Reddit or X); the
  vocabulary is cheap to change before the lists multiply.
* A chip with a single value in the current set is hidden (one country = no Country chip). With four lists at
  launch most chips are therefore hidden: the format is ready, the screen stays calm.
* **Not filters:** ad-free share (lists ignore ads), popularity (no telemetry), date of last change (06 §6), number
  of stations.

### 6.6 Launch lists
Written by the maintainer, 10 to 20 stations each. The French list is in French; the three international lists are in
English (§4). Titles say what is in the list rather than a mood, and avoid
"Pépites" / "Hidden gems", the name of a personal Discover row (09 §5.3).

| id | Title | Use | For | Candidates (to check one by one in Radio-Browser) |
|---|---|---|---|---|
| `fr-community-radios` | Radios associatives en France | discovery, shows | French local and community radios, sponsorship included | Fréquence 7, RDWA, Fréquence Paris Plurielle, Radio Campus stations… |
| `ambient-focus` | Ambient to focus | focus, calm | Background for work, few voices | SomaFM (Groove Salad, Drone Zone…) and others |
| `eclectic-discovery` | Independent stations | discovery | Human programming, independent stations | Radio Paradise, KEXP, WXPN, WFMU, NTS… |
| `public-cultural` | Public radio music channels | discovery, calm | Music channels of public broadcasters, one or two per broadcaster, from many countries | FIP, France Musique, Klara, NPO Radio 4, Rai Radio Classica, NRK Klassisk, ICI Musique… |

At launch the Use chip offers Focus, Calme, Découverte and Émissions; Énergie appears with the first list that uses
it.

No rule caps how many stations one network or broadcaster brings: a list of every FIP channel is a fine list. The
one test, when the maintainer reviews a pull request, is that the stations match the title and the description.

The candidates come from the brainstorm and are not checked: each one needs its Radio-Browser UUID, a working
stream, and a listen. Fréquence Paris Plurielle goes in `fr-community-radios` only (the brainstorm put it in two).

---

## 7. Navigation with lists

06 §1 (three destinations: Listen, Discover, History) was written before lists. Four ways to place them:

**A. Inside Discover.** One row per list among the Discover rows, and an "All lists" screen opened from Discover,
with the filters of §6.5. Keeps three destinations. But Discover then holds two kinds of objects (stations and
lists) with two sets of filters on one screen, which 06 §1 ("filters, not sub-sections") does not allow, so lists
end up one level down.

**B. A fourth destination, Lists.** Listen · Discover · Lists · History.
* **Lists:** the published lists with the filters of §6.5, a list's screen, later the personal lists (v2) next to
  them. "In N lists" in the player and the chips on the station sheet open it.
* **Discover** keeps what works on stations: search, station filters, Surprise me, Scan, rule collections, the
  "No commercial ads" row.
* Four destinations still fit a bottom bar on mobile and a sidebar on desktop; one level each.
* Cost: 06 §1 and 09 §3 change, and Discover loses the hand-picked rows (09 §5.2).

**C. Discover split in two: Stations | Lists.** A two-way switch at the top of Discover, each side with its own
filters; the search box searches the side that is shown.
* **Stations:** what B leaves in Discover. **Lists:** the published lists with the filters of §6.5.
* **Personal lists (v2) go to Listen**, next to favorites. The split becomes: Listen is *mine* (favorites, my
  lists), Discover is *other people's* (the catalog, the published lists), History is *what I heard*.
* "In N lists" and the station sheet chips open Discover on the Lists side.
* Keeps three destinations. Four lists at launch fill one side of a screen, where B would give them a whole
  destination that looks empty (06 §3).
* Cost: a switch inside a destination is the "tab inside a tab" 06 §1 forbids. It needs one written exception, kept
  to Discover and to two sides, so the rule still holds everywhere else. The list screen is one level deeper than
  in B (Discover › Lists › a list), like a station sheet already is.

**D. Lists are Discover's collections, under the same chips.** No switch, no new destination, no new level.
* Discover already shows collections as rows (09 §5): a curated list is one more row, its header giving the title,
  "by @login" and one line of description. Rule collections and lists are the same kind of row; the list screen is
  the row's "See all", one level down, like a station sheet.
* **One set of chips for the whole screen.** The fields of §6.5 were chosen to match the station filters: Genre
  (same 08 §3.1 families) and Country. A chip filters the search results (stations, as in 08) and the rows at the
  same time: a list's row stays when the list matches (its `genres`, `countries`). The Language chip filters
  stations only: lists have no language field (§6.1).
* **Use** is the one chip stations do not have. It filters rows; in search results, it keeps the stations of the
  lists that have this use ("Focus" + typing "jazz" = jazz stations from focus lists).
* Typing in the search box: the matching lists first (title, description, curator), as one row, then the
  stations, as today.
* Personal lists (v2) go to Listen, next to favorites: Listen is *mine*, Discover is *other people's*, History is
  *what I heard* (the split C brought, kept).
* "In N lists" and the station sheet chips open the list screen directly.
* With four lists at launch, Discover gets four more rows among the others: nothing looks empty.
* Cost: nothing in 06 changes. 09 §3 gains the Use chip and the lists among the rows. The Curator filter of §6.5
  becomes text search on the login. Browsing lists as a grid of cards is not offered; if the lists grow to several
  dozen, a "All lists" screen (one level down, same chips) can be added without changing the rest.

**Decided (2026-11-06): D.** It is A without A's flaw: lists and stations share one set of filters instead of each
having its own, so the screen stays one level with chips, as 06 §1 asks. No exception to write, no extra level.
09 §3 and §5.2 are updated; 06 §1 only gains personal lists under Listen.

---

## 8. Order of work
Lists come first: the four launch lists are drafted and their streams checked, while the attribute needs a statement
found station by station.
1. soundfolk-stations: `lists/`, schema, lint, daily check, `v1/lists.json`; the four launch lists (§6.6).
2. App: fetch lists, `lists_for_station`, chips on the sheet, "In N lists" in the player, the list screen, M3U export.
   The reader ignores unknown keys (§6.4), with a test reading a `lists.json` that has one: no
   `deny_unknown_fields` there, unlike the recipe structs.
3. The lists as Discover rows, the Use chip, lists first in search results (§7 D, with the Discover tab of 09).
4. soundfolk-stations: `[ad_free]` section, lint, daily URL check; entries for the known stations (SomaFM, Radio
   Paradise, FIP family, KEXP…), each with its proof.
5. App: read `ad_free` from `v1/stations.json`; badge; the chip and the row come with 08 and 09.
6. v2: personal lists (SQLite, independent of favorites, personal stations allowed since they never leave the
   device), JSON import/export, M3U export.

---

## 9. Still open
* Nothing left on the format. The stations of the four launch lists are picked and their streams answer; nobody has
  listened to them yet (voices in `ambient-focus`, the community status of each station in `fr-community-radios`).
