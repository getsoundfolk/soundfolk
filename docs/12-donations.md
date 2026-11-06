# 🤝 Donations

How Soundfolk shows a station's donation link, and when it suggests donating. Not built yet: this is the plan.

## 1. Goals
* **The player shows the station's donation link** when it has one (06 §2), with the domain it leads to.
* **A discreet reminder** suggests donating to the stations the listener really listens to, at most once a month,
  never during playback, and never in a dialog (06 §9).
* **No wrong link.** A donation link that leads to someone else diverts the listeners' money: every link is
  checked against the station's own website before it is published, and again every day.

What it does not do: take payments, show amounts, or know whether the listener donated. The link opens the
station's page in the system browser, whose address bar shows where the money goes.

## 2. Where the links come from
Radio-Browser has no donation field. The links live in
[getsoundfolk/soundfolk-stations](https://github.com/getsoundfolk/soundfolk-stations): what Soundfolk adds to
Radio-Browser, not a correction layer (stream, tags, country and language stay with Radio-Browser). It stands in
for `soundfolk.fm` (09 §5.2) until that server exists: the app then only changes the list's address.

### 2.1 One file per station
`stations/<id>.toml`, the id being the station's recipe id when it has one:

```toml
#:schema ../station.schema.json
schema = 1
name = "Radio Campus Clermont-Ferrand"
homepage = "https://campus-clermont.net/"
match = { stations = ["<Radio-Browser UUID>"], streams = ["live.radio-campus.org:8002/clermont-ferrand"] }

[donate]
url = "https://www.helloasso.com/associations/…"
found_on = "https://campus-clermont.net/soutenir/"
```

* `homepage`: the station's official website, reviewed by the maintainer. Links are checked against it, never
  against Radio-Browser's `homepage`, which anyone can edit.
* `match`: the recipes' `Match` (07). `match.stations` is required: stations missing from Radio-Browser are not
  accepted for now.
* `donate.found_on`: the page of the station's website that links to `donate.url`, the proof. The bot fills it in.
* `donate.checked_by_hand`, instead of `found_on`, **maintainer only**: when the website doesn't link to the page
  (common with small stations), how and when the maintainer checked it another way (an e-mail from the station's
  address, the association registry). CI refuses it in a pull request from anyone else.
* Nothing else in `schema = 1`. A logo, `ad_free` and `listener_supported` come later, each with its own proof.

Licences: ODbL 1.0 / DbCL 1.0 for the entries, MPL 2.0 for the schema and tooling, like the recipes.

### 2.2 Contributing
From the station sheet, **Suggest a donation link** opens a prefilled GitHub issue form: the listener pastes one
URL and ticks the licence box. A bot checks it and opens the pull request. Only the maintainer merges.

### 2.3 Checks
* `lint.py`: `https://` only, no URL shortener, no tracking parameters; `found_on` on the domain of `homepage`;
  the donation domain is the station's own or one of `platforms.toml` (maintainer only): HelloAsso, Liberapay,
  Ko-fi, Patreon, Tipeee, Open Collective, Donorbox. PayPal is not accepted for now. One entry per station.
* The bot opens a pull request only when the station's website links to the page. HelloAsso's `…/widget` links
  count as the form they embed, and the bot replaces a widget link given as `donate.url` by its form. Otherwise
  it labels the issue *manual-check* and explains why; the maintainer may then add the entry with
  `checked_by_hand`. Every pull request is merged by the maintainer, and the bot labels what to look at:
  *new-station* (is `homepage`, taken from Radio-Browser, official?), *link-change*, *homepage-change*.
* **Every day**, CI checks every published link:
  * an entry checked by hand has no page to read again: only `donate.url` is checked;
  * a link no longer found on `found_on` is **left out of the published list at once**, and comes back by itself
    as soon as it is found again (the files in `stations/` don't change);
  * an issue is opened after 7 days of failure;
  * if more than 20 % of the links (and at least 10) fail the same day, nothing new is left out and one issue is
    opened: the problem is on CI's side (blocked runner, network);
  * a donation URL that redirects to a domain that is not allowed, or a page that no longer answers at all, is
    left out and an issue is opened at once. Parking pages are not detected yet.
  * The state (which links are out, since when) is published next to the list, `v1/status.json`, and read back
    by the next run.

### 2.4 In the app
* `v1/stations.json` (`id`, `name`, `match`, `donate`) is downloaded once a day with `If-None-Match` and cached,
  like the recipes (`recipe_list.rs`). Unknown keys are ignored, so new optional fields keep `v1`.
* Downloaded **even when web titles are off** (⚙ › Privacy): the list is part of the catalog, and GitHub sees the
  listener's IP address at most once a day.
* A station gets `donate: Option<String>` from the first entry whose `match` fits it.

## 3. The link in the player
Next to ♡, a **Donate** link showing the domain in its tooltip (`helloasso.com`). It is always there when the
station has a link, whatever the reminder settings. It opens with `openUrl`, in the system browser. It uses no
heart icon: ♡ already means "like this track".

## 4. The reminder

### 4.1 Which stations
A station with a donation link, listened to for **at least 10 hours in the last 30 days, on at least 3 different
days** with at least 15 minutes each. Days, not sessions: a session ends at every reconnection. Computed from the
listening sessions (03), on the device. Favorites alone don't count.

### 4.2 When
* When playback stops, or at launch (the last station is loaded, not playing yet). Never during playback: the
  banner pushes the player down and the listener who comes back to the window is about to click something.
* **At most one reminder a month**, all stations together.
* The same station again in **6 months** if the listener did not choose anything.
* When the delay of a choice ends, the station must still meet 4.1.

### 4.3 The banner
In the page flow at the top of the home screen, where `ClickNotice` is, above the player. Not floating, not
animated, no sound, no system notification. One banner at a time; the listen-counting notice goes first.

```
┌──────────────────────────────────────────────────────────────────┐
│ You've listened to Radio Campus for 12 h this month.              │
│ It asks its listeners for support.          [Donate]   Ignore      │
└──────────────────────────────────────────────────────────────────┘
```

**Ignore** replaces the buttons, in the banner itself:

```
┌──────────────────────────────────────────────────────────────────┐
│ Remind me: in 1 month · in 6 months                               │
│ I already donated · Not for Radio Campus · No more reminders      │
└──────────────────────────────────────────────────────────────────┘
```

| Choice | Effect |
|---|---|
| **Donate** | Opens the page. No reminder for this station for 1 year. |
| **In 1 month** / **In 6 months** | The next reminder for this station, at the earliest. |
| **I already donated** | Like Donate: 1 year. |
| **Not for <station>** | Never again for this station; can be undone in the settings. |
| **No more reminders** | Turns the setting off. |
| Nothing (playback resumes) | Counts as seen: 6 months for this station. |

The banner does not fade by itself: it waits for a choice or for playback. It says true facts only: the
listener's hours, and that the station asks for support (true of every station with a link). It never makes up
figures such as how many listeners donate.

### 4.4 Settings
* **Donation reminders**, on by default.
* Below it, the stations set to "Not for <station>", each with **Allow again**. Hidden when empty.

### 4.5 Data
A local table `donation_prompts(station_id, next_at, never)`; the date of the last reminder shown (for the monthly
limit) goes in the settings. Nothing is sent. All the figures of this section are constants in one place.

## 5. Where the figures come from
No public study covers donation reminders in a radio app. The figures follow what others publish about their own
appeals, which are reports, not independent measures:

* Wikipedia: the first two banner impressions bring most donations, so it shows 1 to 3 per reader per year
  ([2016-17](https://meta.wikimedia.org/wiki/Fundraising/2016-17_Report),
  [2013-14](https://meta.wikimedia.org/wiki/Fundraising/2013-14_Report)); banners are hidden 250 days after a
  donation ([T121178](https://phabricator.wikimedia.org/T121178)); its apps offer "I already donated" and a
  reminder the reader schedules ([T346823](https://phabricator.wikimedia.org/T346823)); facts beat personal
  appeals ([2012](https://meta.wikimedia.org/wiki/Fundraising_2012/Report)).
* Signal: at least 30 days between two donation prompts
  ([Megaphones.java](https://github.com/signalapp/Signal-Android/blob/main/app/src/main/java/org/thoughtcrime/securesms/megaphone/Megaphones.java)).
* The Guardian: asking right after a story is read works best
  ([Nieman Lab](https://www.niemanlab.org/2024/01/how-the-guardian-raised-a-record-amount-of-reader-revenue-in-the-u-s/)).
* A more prominent donate button did not convert better
  ([FundraiseUp](https://fundraiseup.com/donor-experience-hub/reminder-element-button-design/)).

Since nothing is measured on the listeners' side, the only feedback is theirs. If reminders feel too frequent,
the monthly limit is the first figure to change.

## 6. Order of work
1. `soundfolk-stations`: `station.schema.json`, `lint.py`, `platforms.toml`, `CODEOWNERS`, the CI (lint, link
   check, compile, publish on Pages, daily check), the issue form and its bot.
2. The app downloads the list and gives each station its `donate` link.
3. The link in the player.
4. **Suggest a donation link** in the station sheet.
5. The reminder: listening-time rule, banner, settings, `donation_prompts`.
