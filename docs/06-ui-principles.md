# 🧭 UI Principles

How Soundfolk's interface is organised, for the current desktop client and the future mobile clients. The interface
follows what a listener wants to do — **listen, find, find again** — not the shape of the catalog or the database.
Colors, type, icons and spacing are in [07-visual-design.md](07-visual-design.md).

These principles come from reviewing other open radio players. AMARadio (Android), for example, exposes its data model
directly: unlabelled icons, tabs named "Local", "Filter" and "…", empty screens asking to configure a filter, bitrate,
codec and click counts on every row, and a "Update database" button in the settings. The rules below avoid each of
these.

---

## 1. Navigation
* **Three labelled destinations**, no more:
  * **Listen:** resume the last station, then **Favorites** (catalog stations, personal stations and curated lists
    marked ♡, together) and **My stations** (the stations added by hand, where they are added and removed), always
    in that place; later the listener's own lists ([15-curated-lists.md](15-curated-lists.md) §7);
  * **Discover:** search, filters and collections, published curated lists included, under one set of chips;
  * **History:** tracks heard, likes and stats.
* Settings live in a menu, not in the main navigation.
* **One navigation level.** No tabs inside tabs, no icon bar repeating the same actions. A screen may have filters, not
  sub-sections.
* Every icon has a label (or at least a tooltip on desktop). No "…" tab, no unexplained "+".

## 2. Playback first
* The app **opens ready to play**: the last station is loaded in the player, one tap or click away.
* The player always shows **the station and the track on air**, with ♡ (like) and the station's donation link when it
  has one. A player showing only the station name says nothing.
* **Playback resumption (mobile):** the last station stays in the system media controls (Android Quick Settings /
  lock screen, iOS Now Playing) after the app is closed, and **Play there starts it** without opening the app.
  On Android this is Media3's `onPlaybackResumption` (Android 13+). It is **on by default** and can be **turned off**
  in the settings ("Show the last station in system media controls"), for listeners who don't want it visible there.
  Turning it off removes the station from those controls at once — never "restart your device".

## 3. No empty screens
* A screen never asks for work before showing something. "Discover" opens on **curated collections** (e.g.
  "Ad-free, listener-supported", "French community radios", "Jazz") and on stations from the listener's country or
  language — not on an empty filter to configure. See [10-discovery.md](10-discovery.md).
* Empty favorites or history show one sentence and a way forward ("Like a track with ♡ to find it here", a link to
  Discover), not "No results".

## 4. Filters on the results
* Filters are **chips above the results** (Genre, Country, Language, Ad-free, Listener-supported, Working now), applied
  immediately and combinable. No separate filter screen, no saved "filter feeds" to set up first.
* Search and filters work together: typing narrows what the chips already selected.

## 5. Human information first, technical details on demand
* A station row shows: **name, genres, country**, and only useful badges — *ad-free*, *listener-supported*, *HD*
  (high bitrate or lossless), *personal*, and a warning when the stream is down.
* Bitrate, codec, stream URLs, HLS, Radio-Browser tags and click counts belong to the **station sheet** ("More
  details"), never to lists.
* Popularity is never the default sort of the home screen: it would put the biggest commercial networks first, the
  opposite of what Soundfolk stands for. Default order: curated, then relevance, then working streams first.

## 6. No visible maintenance
* The listener never sees a database: no station count, no "last updated", no "update the database" button. The
  catalog is kept current server-side and fetched as needed.
* Platform constraints (battery optimisation, notification permission) are handled when they matter — at the first
  background playback — with one explanation and the button that opens the right system screen. Not as a permanent
  warning in the settings.
* No prompts unrelated to listening (GitHub stars, ratings) in the main screens.

## 7. Language
* **Everything is translated**, genres included. Raw tags (`aac,news,talk`) are never displayed; they are mapped to
  translated genres.
* Words a listener uses: "My stations" rather than "Custom", "Working now" rather than "lastcheckok", "Unknown" only
  when there is genuinely nothing to show — and then nothing is shown instead.

## 8. One list per concept, one layout per list
* **Favorites** contain catalog and personal stations together, and the curated lists marked ♡; personal stations
  carry a small *personal* badge. Lists and stations mixed in one screen are filtered with a type chip (All ·
  Stations · Lists), not split into tabs, and only once there are enough lists for it to matter (§4).
* A list keeps its own order (the curator's, or the listener's); chips filter it, they never re-sort it.
* A given list always uses the same layout. Grid vs. list is a single, global choice, if offered at all.

## 9. Respect while listening
* Donation suggestions ([13-donations.md](13-donations.md) §4): shown when playback stops or at the next launch,
  never during playback, never modal, at most once a month, and they can be turned off for good.
