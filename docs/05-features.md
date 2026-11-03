# 🚀 Feature Catalog

This document lists the functional and technical features of **Soundfolk** (`soundfolk.fm`), from the local MVP
onwards.

---

## 1. 🎵 Audio core & player
* **Continuous streaming (ICY / HTTP):** smooth playback of web radio streams over standard broadcast protocols.
* **Live metadata:** the artist and title currently on air, extracted from ICY metadata and recorded in the listening
  history.
* **Resilient network buffer:** absorbs network micro-cuts; automatic reconnection if the stream drops.
* **Fast start:** measured at roughly half a second to the first decoded sound on an Icecast stream (network-bound).
* **Background playback:**
  * Desktop: 100 % Rust engine (`symphonia` + `cpal`).
  * Mobile (future): native `AVPlayer` (iOS) and `ExoPlayer/Media3` (Android).

---

## 2. 🔍 Catalog & discovery
* **Global coverage via Radio-Browser:** worldwide catalog (independent and commercial web radios), searched online by
  name, with a short typing delay to spare the API.
* **Honest station health:** stations whose last check failed stay visible with a warning (the check can be a false
  negative) and are ranked after healthy ones; HLS stations are listed but disabled until they are supported.
* **Add a station by URL:** direct stream, `.m3u` or `.pls`. The stream is tested before it is saved. Private to the
  device by default; nothing is sent anywhere.

---

## 3. ⭐ Personal curation
* **Favorites:** quick pinning of preferred stations, stored locally in SQLite.
* **Listening history:** automatic, timestamped record of the tracks played on air, so a discovery is never lost.
* **Personal stations:** your own streams, kept alongside catalog stations (marked "personal").

---

## 4. 🔒 Privacy
* **Listening count (on by default):** when a catalog station starts playing, Soundfolk tells Radio-Browser, which uses
  it for popularity rankings. It sends the station UUID and, as with any request, the IP address — nothing else. A short
  notice appears at the first play (that first play is not counted), and the setting can be turned off at any time.
  Personal stations are never reported.
* **No telemetry** beyond that.
