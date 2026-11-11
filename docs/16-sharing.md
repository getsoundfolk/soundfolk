# 🔗 Sharing

How a listener sends a station, a track on air or a list to a friend, on desktop and on mobile, and how that link
brings new listeners to Soundfolk. Not built yet: this is the plan.

## 1. Goals
* **One link, pasted anywhere.** A link that works in a chat app, an email or a forum post, clickable everywhere.
* **A friend with Soundfolk lands on the station or the list**, one click or tap away, on desktop as on mobile.
* **A friend without Soundfolk can listen right away** in the page (§5.2), then gets the app in one step. This is
  where sharing brings new listeners: the page is the first thing they see of Soundfolk.
* **The moment is worth more than the station.** "*Nina Simone — Sinnerman* on FIP" makes people click; "FIP" alone
  rarely does (§4.1).
* **Nothing stored anywhere.** Soundfolk has no accounts and no server (15 §3.3). The link carries everything; the
  page it points to is static and never sees what is shared.
* **A link is data from a stranger.** Nothing it carries runs, plays or is saved without the listener's click, and
  nothing it carries can reach the listener's local network (§6).

What it does not do: shared lists that stay in sync (a shared list is a copy), "who shared what", counters, short
codes, or tracking whether a link was opened. All of these need a server, and the last one would go against what
Soundfolk is (§7.4).

---

## 2. The link

### 2.1 Where it points
`soundfolk.fm` is not registered yet. Until it is, links point to the organisation's GitHub Pages site,
`https://getsoundfolk.github.io`, served by the repository `getsoundfolk/getsoundfolk.github.io`. The format below
depends only on the path and the fragment, so the host can change later (§8) without breaking a link.

### 2.2 Format
| What | Link |
|---|---|
| Catalog station | `https://getsoundfolk.github.io/s/#<Radio-Browser UUID>` |
| Personal station | `https://getsoundfolk.github.io/s/#n=<name>&u=<stream URL>` |
| Track on air | a station link with `t=<artist — title>`: `…/s/#id=<UUID>&t=…` or `…/s/#n=…&u=…&t=…` |
| Published list | `https://getsoundfolk.github.io/l/<list id>/` |
| Personal list (15 §8, v2) | `https://getsoundfolk.github.io/l/#n=<title>&s=<stations>&p=<station>&p=…` |

Any link may also carry `f=<name>`, who sent it (§4.2).

* **Why the fragment.** A browser never sends the fragment (`#…`) to the server, so GitHub never sees what is shared,
  and neither do the chat apps that fetch a link to preview it. GitHub Pages only serves files: the fragment links
  need two of them, `/s/index.html` and `/l/index.html`; a path like `/s/<uuid>` would be a 404.
* **Why a path for published lists.** Chat apps build their preview from the page's Open Graph tags and never see the
  fragment, so every fragment link previews the same generic card (§5.4). Published lists are public anyway: each gets
  a real page with its own preview, generated from soundfolk-stations (§5.4). `/l/#<list id>` is accepted too.
* **Key-value fragments** are `application/x-www-form-urlencoded`. Unknown keys are ignored. A fragment without
  `=` is an id (`id=` spelled out once other keys follow). `v=<n>` is reserved for a future format; absent means 1.
* **`s` of a personal list:** the stations' UUIDs as 16 raw bytes each, concatenated, in base64url without padding
  (22 characters per station, 880 for 40 stations). Compression gains nothing on random UUIDs.
* **Personal stations in a personal list** go in the link, since they are often the stations a list is worth sharing
  for:
  * **Catalog first.** When sharing, each personal station's stream URL is looked up in Radio-Browser (`byurl`);
    one found there goes in `s` as its UUID, like any catalog station.
  * **The others** are one `p` key each, repeated in the list's order: the name, a line feed, the stream URL, all
    URL-encoded (about 100 characters per station). The list's order across `s` and `p` is lost: catalog stations
    come first. Keeping it would need a position per station, which is not worth the length.
* **Limits**, as for published lists (15 §6.1): title 40 characters, 1 to 40 stations in all (`s` and `p`), no UUID
  twice. A personal station's name: 80 characters; its stream URL: `http` or `https` only, no login
  (`https://radio.fr@other.example/` goes to `other.example`), 2,000 characters. `t`: 200 characters. `f`: 40
  characters. Counted after cleaning (§6.1). The whole link: 16 KiB. A link breaking a limit is refused whole, never
  cut. The app checks a link it writes the same way before showing it, so it never hands out one it would refuse.

### 2.3 The app's scheme
Desktop apps cannot claim `https` links (§3), so the app also registers `soundfolk://`. A `soundfolk://` link is the
`https` link with its origin replaced: `soundfolk://s/#<uuid>`. It is never what the listener copies: only the
page's button (§5.3) uses it.

### 2.4 One parser
A single Rust function turns a link into a `SharedItem` (catalog station, personal station, published list, personal
list, each with its optional track and sender) or an error. It accepts the `https` link on every known host (§8),
the `soundfolk://` link, and the bare id (a UUID pasted alone). It cleans and checks every field (§6.1) before
anything else sees it. Desktop and mobile both use it; the page has a JavaScript copy tested against the same cases.

---

## 3. Opening a link

### 3.1 Desktop
A desktop OS opens an `https` link in the browser, never in an app. Three ways in:

1. **The page's button.** The page (§5.3) shows **Open in Soundfolk**, a `soundfolk://` link. The browser asks once
   whether to open Soundfolk.
2. **Paste into the search.** Discover's search field recognises a pasted link (§2.4) and opens what it carries
   instead of searching. Nothing to install, works everywhere, including when the scheme is not registered.
3. **Drop on the window.** A link dropped from the browser does the same as a paste.

The scheme is registered by `tauri-plugin-deep-link` (`plugins.deep-link.desktop.schemes = ["soundfolk"]`). On
Linux the `.deb` declares `x-scheme-handler/soundfolk` in its `.desktop` file, and the app registers it again at
every launch (`register_all()`), which covers the AppImage and development builds from their first run. What is left
is a friend without Soundfolk, or with an AppImage never run: §5.3 handles it. `tauri-plugin-single-instance`,
already used, gets its `deep-link` feature so that a link reaches the running window instead of starting a second
Soundfolk (§6.3).

### 3.2 Mobile (future)
The `https` link opens the app directly:
* **iOS:** Universal Links, through `/.well-known/apple-app-site-association` on the site.
* **Android:** App Links, through `/.well-known/assetlinks.json` on the site.

Both files live in `getsoundfolk.github.io` with a `.nojekyll` file at its root, without which Jekyll skips
dot-directories. `tauri-plugin-deep-link`'s mobile configuration lists the host. **To check when mobile starts:**
GitHub Pages serves `apple-app-site-association` (no extension) as `application/octet-stream`; Apple's CDN is
believed to accept it, but this has not been tried.

### 3.3 In the app
A link never starts playback by itself: a link is something someone else chose. One link at a time: a new link
replaces the sheet of the previous one (§6.3).
* **Sender and track first.** "From Raph" when the link has `f`, then the track when it has `t`: "Raph was listening
  to *Nina Simone — Sinnerman*". The track is what was on air then, not now: the player shows what is on air once ▶
  is pressed.
* **Catalog station:** opens its station sheet, with ▶ and ♡. A UUID gone from Radio-Browser shows "This station is
  no longer in the catalog", with the UUID.
* **Personal station:** a sheet with the name and the stream's domain, and **Add to My stations**, which goes through
  "Add a station by URL" (05 §2), stream test included, with the restrictions of §6.2. If the stream URL is a catalog
  station's (Radio-Browser's `byurl`), the sheet offers the catalog station instead.
* **Published list:** opens the list's screen (15 §6.4). A list id the app does not know may have been published
  since the app's daily fetch: the app fetches `lists.json` again, once, before saying "This list is not published"
  (removed, or not yet merged).
* **Personal list:** a preview ("*Late-night jazz* · 12 stations") with the stations, and **Add to my lists**, which
  stores a copy. Adding it twice asks whether to replace or keep both.

---

## 4. Sending

### 4.1 Where
**Share** sits where the thing is:
* **The track on air**, in the player, and **a track of the History**: the link to its station with `t` (§2.2). This
  is the share most likely to be clicked, and the one that comes up naturally while listening.
* **A station:** the station sheet, the player's station menu.
* **A list:** the list's screen.

On mobile, Share opens the system share sheet with the `https` link and a one-line text ("*Nina Simone — Sinnerman*
on FIP · Soundfolk"). On desktop, a small popover with:
* **Copy link** (the `https` link). Copy says "Link copied" in place, no toast.
* **A QR code** of the same link, to send it from the computer to a phone. Error correction level L (the screen is
  clean, every module counts); it grows with the link, at least 4 px per module: a 40-station list (about 950 bytes,
  version 25, 117 modules) is about 470 px. Whether phones scan that from a laptop screen is still to try (§10).

### 4.2 Sender
A name in ⚙, **Your name in shared links**, empty by default. When set, links carry it as `f`. A link from someone
the friend knows is opened more often; the name is only what the sender typed, shown as "From …", never as a
verified identity (§6.1).

### 4.3 What leaves the device
Said in the popover, in one line, when it applies:
* **Personal station:** "The stream address goes in the link." A personal station otherwise never leaves the device
  (03 `personal_stations`), so sharing one is the listener's explicit choice.
* **Personal list with personal stations:** "The addresses of 3 stations added by hand go in the link, and are
  looked up in Radio-Browser." Those found in the catalog (§2.2) are not counted in the first part.
* **The sender's name** is in every link while it is set; the setting says so.

Favorites and My stations are not shared as such; once personal lists exist (v2), a list can be made from them.

---

## 5. The page

### 5.1 Shape
`/s/index.html`, `/l/index.html` and the generated list pages (§5.4): plain HTML and a little JavaScript, no
framework, no third-party script, no analytics, no cookies, the security rules of §6.4.
* It parses the fragment like the app (§2.4).
* **Catalog station:** fetches the name from Radio-Browser (`/json/stations/byuuid`), which serves CORS. This sends
  the UUID to Radio-Browser, as the app itself does. No favicon: loading it from the station's website would tell
  that website the visitor's address before they chose to listen.
* **Published list:** title, description, "by @login" for its curators, and its stations' names, written into its
  page at build time (§5.4).
* **Personal station or list:** the name or title from the fragment, as text, never as HTML.
* **Sender and track** as in the app (§3.3), as text.
* An invalid fragment says "This link is incomplete" and still offers Get Soundfolk.

### 5.2 Listen in the page
For a friend without Soundfolk, listening right away is what turns a link into a listener, and on mobile, until the
app exists, it is the only thing the page can give.
* **Catalog stations only**, from Radio-Browser's `url_resolved`, never a URL from the fragment (§6.4).
* **`https` streams only:** a page served over `https` cannot play `http` (mixed content). A stream the catalog gives
  as `http` is tried over `https` at the same address (many servers have both; not when the address names a port):
  by the build for published lists, at the click for a station. The address is still the catalog's. For the
  others, the page says "Plays in the Soundfolk app". In November 2026, 49 of the 59 stations of the four
  published lists play in the page, 33 without the `https` try.
* **The browser's own `<audio>`**, codecs it supports (MP3, AAC, Ogg). No HLS (05: not supported yet), no
  now-playing, no loudness: the reasons to get the app (15 §3.3 lists them).
* **Never starts by itself**; ▶ starts it. The page does not count a click in Radio-Browser (the app asks first,
  the page cannot).
* A published list's page plays its stations the same way, one at a time.

### 5.3 Getting to the app
* **Open in Soundfolk** (`soundfolk://…`), **Copy link** (for the search, §3.1) and **Get Soundfolk**.
* **Get Soundfolk** depends on the visitor's system (the latest release is written by the build in
  `/release.json`: the page asks nothing of GitHub's API): on Linux, the install command or package for their
  distribution once there is more than the `.deb` (§7.3), else the `.deb` of the latest Release; on other systems,
  "Soundfolk runs on Linux for now", the Releases page, and listening in the page. Every download is a GitHub
  Release asset, which has a build provenance attestation.
* **A new release rebuilds the site.** The `site` job of `release.yml`, on a `v*` tag once the release is
  published, starts the `Pages` workflow of `getsoundfolk.github.io`. Without it the download would wait for the
  site's daily build. The job needs `SITE_TOKEN`, made by hand by a maintainer: a fine-grained token, owner
  `getsoundfolk`, repository `getsoundfolk.github.io` only, permission "Actions: Read and write", with an expiry
  date. It can start the site's workflows and nothing else: it cannot push to the site. Then:
  ```bash
  gh api -X PUT repos/getsoundfolk/soundfolk/environments/site \
    --input - <<< '{"deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}'
  gh api -X POST repos/getsoundfolk/soundfolk/environments/site/deployment-branch-policies \
    -f name='v*' -f type=tag
  gh secret set SITE_TOKEN --env site --repo getsoundfolk/soundfolk
  ```
  The environment `site` allows deployments from tags `v*` only, as `apt` and `winget` do. Until the secret is
  set, the job says so and stops; when the token expires, it fails: make a new one and set it again.
* **When `soundfolk://` is not registered** the browser does nothing, and a page cannot tell. A hint instead: if
  the page has not lost the focus about 2 s after Open in Soundfolk, it shows "Nothing happened? Get Soundfolk, or
  paste the link in its search." The browser's own "Open Soundfolk?" prompt also takes the focus, so the hint can
  stay hidden when it was needed, or show when it was not; either way it costs nothing.

### 5.4 Previews in chat apps
* **Fragment links** (stations, personal lists) all preview one card: "Soundfolk · Web radios from around the
  world", with a designed image. Chat apps never see the fragment, so it cannot be better without a server; it can
  at least look good.
* **Published lists** get a real page each, `/l/<list id>/index.html`, with `og:title`, `og:description` (the list's
  description, then "by @login") and an image naming the list and its curators. A workflow in
  `getsoundfolk.github.io` builds them every day from `v1/lists.json` of soundfolk-stations (15 §6.3).
  * **Station names and streams from Radio-Browser at build time.** `lists.json` only has each item's UUID and its
    stream without scheme. The workflow asks Radio-Browser for the whole list in one request
    (`POST /json/stations/byuuid`) and writes names and `url_resolved` into the page, so the page needs no request
    to show the list, and listening in the page (§5.2) knows which streams are `https`.
  * **A list published since the last build:** the site's `404.html` sends an unknown `/l/<id>/` to `/l/#<id>`,
    which reads `lists.json` in the browser.
  * **A removed list keeps its page**, reduced to "This list is no longer published" with a link to the published
    lists. The build publishes the ids it has ever built with the site (`/lists-seen.txt`) and reads them back the
    next time, to know which ones to turn into such a page: the workflow writes nothing to the repository, whose
    `main` stays protected (§6.5). A link someone posted years ago still says something.

---

## 6. Security

### 6.1 What a link carries
Everything in a link, and everything Radio-Browser says about a station (anyone can edit it), is untrusted text.
* **Shown as text**, never as HTML, in the app and in the page. Nothing from a link becomes a clickable link.
* **Cleaned before use**: Unicode NFC; control and format characters removed (C0, C1, zero-width characters, the
  bidirectional overrides and isolates U+202A–U+202E and U+2066–U+2069), so a name cannot hide text or show a fake
  domain; whitespace collapsed. Limits (§2.2) count the cleaned text.
* **The stream's domain** is shown next to a personal station's name: the name says what the sender claims, the
  domain where the sound comes from.
* **The sender** is shown as "From …", plain, never with a badge or anything that suggests it was checked.

### 6.2 Stream addresses and the local network
A stream URL from a link makes the listener's computer connect where the sender chose. Without a rule, a link could
point at `http://192.168.1.1/…` or `http://localhost:…` and make the stream test send a request to the listener's
router or a local service.
* **Refused** for any stream that came from a link: loopback, private (10/8, 172.16/12, 192.168/16), shared
  (100.64/10), link-local (169.254/16, fe80::/10), unique local (fc00::/7), unspecified and multicast addresses, and
  the names `localhost`, `*.local`, `*.localhost`, `*.internal`, `*.home.arpa`.
* **Checked on the addresses**, after DNS resolution and at each connection, not on the URL alone: otherwise a name
  that resolves to a public address for the check and a private one afterwards (DNS rebinding) gets through. The same
  check applies to each redirect and to each URL inside a `.m3u` or `.pls`.
* **Catalog stations too**, link or not: Radio-Browser's URLs are written by anyone, and a link makes such a station
  easy to aim at someone.
* **Station logos and track covers too**, fetched by the app: favicons come from Radio-Browser and covers from
  station pages. A failure to read whether a station came from a link counts as "from a link".
* **A station added from a link keeps the rule** (`personal_stations.origin = 'link'`). The listener lifts it by
  typing the same address in "Add a station by URL" (05 §2): a local radio typed by hand stays possible.
* **Through a proxy** (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`), the proxy resolves the names and is often on the
  local network itself: only the names and IPs written in the addresses are checked then.
* **Not covered yet:** the streaming server's status page read for the track on air (08), on the stream's own host.

### 6.3 The `soundfolk://` handler
Any program or page can open a `soundfolk://` link, as often as it likes.
* **Passive:** a link opens a sheet; nothing plays, is added or is fetched (beyond the station's name and the
  `byurl` lookup) until the listener clicks.
* **One sheet at a time:** a new link replaces the previous one; a burst of links shows the last.
* **Strict arguments:** from the arguments single-instance forwards, only one starting with `soundfolk://` is read,
  through the parser (§2.4); nothing is taken as a file path or an option.

### 6.4 The page
* **Content-Security-Policy** in a `<meta>` tag: `default-src 'none'`; scripts, styles and images from the site
  only; `connect-src` to Radio-Browser's API; `media-src https:` (streams are on any host); no inline script, no
  `eval`.
* **Plays catalog stations only** (§5.2): a page that played a URL from the fragment would let anyone make a link
  that plays anything under Soundfolk's name.
* **No third-party script**, no web font from elsewhere, no favicon from stations (§5.1).

### 6.5 The site and its accounts
A changed page could hand out a fake download.
* The GitHub organisation requires two-factor authentication; `getsoundfolk.github.io` has a protected `main`,
  changes through pull requests, pages published by its workflow only.
* **Get Soundfolk** only points to GitHub Release assets, whose provenance can be checked
  (`gh attestation verify`).
* When `soundfolk.fm` comes (§8): renewal on automatic with a reminder, DNSSEC if the registrar offers it, and no
  DNS record left pointing at a service no longer used (a dangling `CNAME` lets someone else serve the domain).

### 6.6 What leaves the device
* **Sharing a personal station or list** puts its stream URLs in the link and sends them to Radio-Browser for the
  catalog lookup (§4.3): the listener is told in the popover. 03 `personal_stations` notes this exception.
* **Opening a link**: the station's UUID or the stream's URL goes to Radio-Browser (name, `byurl`), as when
  browsing.
* **The page**: the UUID goes to Radio-Browser; nothing goes to GitHub beyond the path (`/s/`, `/l/` or a published
  list's id). Listening in the page connects to the station's stream server, after a click.

---

## 7. Beyond the link
Sharing between friends only spreads as far as the friends can install the app, and stations reach more people
than any listener.

### 7.1 "Listen on Soundfolk" for stations
A button that stations put on their website, linking to their station link (§2.2). Stations have audiences of their
own, and the donation links (13) make them allies: Soundfolk sends listeners and money their way.
* An SVG badge in a few sizes and both themes, and the HTML to paste, on a page of the site (`/stations/`).
* Offered to the stations of soundfolk-stations when their donation link is added (13 §2), never by unsolicited
  mail.
* The link is an ordinary station link: nothing to build in the app.

### 7.2 Curators
Whoever spent evenings choosing a list wants to show it, and their audience is the right one.
* The page and the preview credit them: "by @login" (§5.4).
* When their pull request is merged, the bot's comment gives the list's link and its preview image, ready to post.

### 7.3 Installing in one step
A friend convinced by a link has to install Soundfolk without hunting for a `.deb`. In order of reach on Linux:
1. **Flathub**: every distribution, updates through the software centre. The first one to do.
2. **AUR** (Arch and derivatives).
3. An APT repository for Debian and Ubuntu, so that the `.deb` updates itself (17).

Windows: winget (18). macOS: Homebrew, now that the build exists. The page's Get Soundfolk (§5.3) follows this list.

### 7.4 Measuring
Not measured. No tracking parameter in links, no counter on the page, no analytics: Soundfolk's promise is that
listening stays private, and a share link is listening. The only numbers are the ones the platforms give anyway:
Release downloads, Flathub installs.

---

## 8. When `soundfolk.fm` comes
* The pages on `getsoundfolk.github.io` become redirects:
  `location.replace("https://soundfolk.fm" + location.pathname + location.hash)`. The fragment follows, so old links
  keep working.
* The app accepts both hosts from the start (§2.4): only the path and the fragment matter, the host is checked
  against a list of two.
* On mobile, both hosts are declared, so old `github.io` links still open the app.
* New links use `soundfolk.fm` from the release that switches.
* The security of §6.5 applies to the domain from the day it is bought.

---

## 9. Order of work
1. Parser (§2.4) with its tests: every form of §2.2, cleaning (§6.1), invalid fragments, limits.
2. Stream address rule (§6.2), for catalog stations too: useful even before sharing.
3. Desktop: paste and drop into the search (§3.1, 2–3), the sheets of §3.3. Already useful without any page: a
   friend can paste the link.
4. Desktop: Share on the track on air, the History, the station sheet; popover with Copy link and the QR code; the
   sender's name in ⚙ (§4).
5. `getsoundfolk.github.io`: the two pages with listening in the page and Get Soundfolk (§5.1–5.3), the
   Content-Security-Policy (§6.4), the generic preview (§5.4), protected repository (§6.5).
6. Desktop: `soundfolk://` with deep-link and single-instance (§3.1, 1; §6.3); `.deb` registration.
7. Flathub (§7.3).
8. Published lists: links, generated and kept pages once 15 is built (§5.4); the curators' links (§7.2); the
   station badge (§7.1); personal lists with 15 §8 step 6.
9. Mobile: share sheet, Universal Links and App Links (§3.2).

---

## 10. Still open
* **QR code of a long list** (§4.1): generate the QR code of a 40-station link at the planned size and scan it from a
  laptop screen with two or three phones. If it fails, the popover shows the QR code only below a length (to be
  found by the same test) and Copy link alone above it.
* **How many catalog stations stream over `https`** (§5.2): if few do, listening in the page helps less than hoped;
  count it in Radio-Browser's data before building §5.2.
