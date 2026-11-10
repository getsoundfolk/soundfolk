# 💾 Local Database Specification

## 1. Data strategy
* **Catalog:** the global station catalog comes from **Radio-Browser** and is queried online through `CatalogSource`.
  There is no full local mirror: without a network no radio can be played, so offline catalog search has no value.
* **Local persistence (SQLite):** the user's favorites, listening history, likes, personal stations and settings, and
  what the app learned from them (MusicBrainz matches, covers, station loudness), with 0 ms responsiveness.
* **Station IDs:** `station_id` (favorites, history) is the Radio-Browser UUID for a catalog station, or
  `user:<hex>` for a personal station.

## 2. Local tables (`rusqlite`, WAL mode)

The schema is versioned (`PRAGMA user_version`): `db/mod.rs` holds the version 1 schema, frozen, and a list of
migrations that only grows, each run in its own transaction. The tables below are as they stand after the last
migration. `station_reports`, created by version 1, was dropped by version 4: nothing ever wrote to it.

### `favorites`
Stations pinned by the user. `position` is the order the listener gives them, 0 first (version 6).
```sql
CREATE TABLE IF NOT EXISTS favorites (
    station_id TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    stream_url TEXT NOT NULL,
    genre      TEXT,
    added_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
    position   INTEGER NOT NULL DEFAULT 0
);
```

### `listening_history`
Tracks played on air, recorded from the stream's metadata or from the station's web page (consecutive duplicates are
skipped; see [08-now-playing.md](08-now-playing.md)).
`station_name` is copied at listening time, because catalog stations are not stored locally: without it, a catalog
UUID could not be shown by name. Rows from before this column have it `NULL`; the app then falls back to the favorite
or personal station of the same ID, if any.

`track_title` is the raw title and is never altered. `artist`, `title` and `track_key` are parsed from it (`tracks.rs`:
station signature removed, then `Artist - Title` or `Title by Artist`). When the parsing rules change, the
`title_parser_version` setting no longer matches and every row is re-parsed at startup. Rows whose source gave the
artist and title apart (station page, Vorbis comments) have `title_source = 'source'`: they are stored as given, with
the real start time in `played_at`, and never re-parsed. `track_key` ignores case,
accents, punctuation, a leading "The", featured artists and bracketed noise such as `(Remastered)`: two spellings of the
same track share it.
```sql
CREATE TABLE IF NOT EXISTS listening_history (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    station_id   TEXT NOT NULL,
    station_name TEXT,
    track_title  TEXT NOT NULL,
    artist       TEXT,
    title        TEXT,
    track_key    TEXT,
    title_source TEXT,
    played_at    DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### `likes`
One row per liked track (by `track_key`). Self-contained, so clearing the history keeps the likes. The station is the one
the track was first liked on, which the stats need.
```sql
CREATE TABLE IF NOT EXISTS likes (
    track_key    TEXT PRIMARY KEY,
    artist       TEXT,
    title        TEXT NOT NULL,
    raw_title    TEXT NOT NULL,
    station_id   TEXT NOT NULL,
    station_name TEXT,
    history_id   INTEGER,
    liked_at     DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### `listening_sessions`
When each station was actually playing, for listening time and like rates (a title in the history only says it was
announced). A session opens when playback starts, continues across a reconnection to the same station, and closes on
stop, error, station change or app exit. `last_seen_at` is updated every minute: a session cut by a crash is closed at
the next startup with `ended_at = last_seen_at`. Clearing the history deletes the sessions too. A partial index on
`ended_at IS NULL` finds the open session without scanning the table (version 8).
```sql
CREATE TABLE IF NOT EXISTS listening_sessions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    station_id   TEXT NOT NULL,
    station_name TEXT,
    started_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_seen_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    ended_at     DATETIME
);
```

### `tracks`, `artists`, `albums`
MusicBrainz identification of liked tracks (opt-in, see the `identify_tracks` setting). `tracks` is keyed like `likes`
and doubles as the work queue: a liked track without a row, or `pending` with `next_attempt_at` in the past, is looked
up next. A failed lookup is retried after 2, 4, 8… minutes (up to a day); a track without a trustworthy match is
`not_found`. Albums are release groups ("the album", whatever the edition), preferring a plain album over a
compilation or a live recording. `tags` and `genres` are JSON arrays.
```sql
CREATE TABLE IF NOT EXISTS tracks (
    track_key          TEXT PRIMARY KEY,
    status             TEXT NOT NULL CHECK (status IN ('pending', 'matched', 'not_found')),
    attempts           INTEGER NOT NULL DEFAULT 0,
    next_attempt_at    DATETIME DEFAULT CURRENT_TIMESTAMP,
    recording_mbid     TEXT,
    title              TEXT,
    artist_mbid        TEXT,
    album_mbid         TEXT,
    first_release_date TEXT,
    tags               TEXT,
    looked_up_at       DATETIME
);
CREATE TABLE IF NOT EXISTS artists (
    mbid TEXT PRIMARY KEY, name TEXT NOT NULL, country TEXT, genres TEXT, looked_up_at DATETIME
);
CREATE TABLE IF NOT EXISTS albums (
    mbid TEXT PRIMARY KEY, title TEXT NOT NULL, primary_type TEXT
);
```

### `covers`
Cover art found online for a track, keyed like `likes`; `url` is `NULL` when none was found. Each track is looked up
once, and again after a while when nothing was found (`covers.rs`).
```sql
CREATE TABLE IF NOT EXISTS covers (
    track_key    TEXT PRIMARY KEY,
    url          TEXT,
    looked_up_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### `station_loudness`
The loudness of each station heard, learned by Constant volume, keyed by the Radio-Browser UUID, else the stream URL.
Cleared with the history. See [11-loudness.md](11-loudness.md).
```sql
CREATE TABLE IF NOT EXISTS station_loudness (
    station_key   TEXT PRIMARY KEY,
    loudness_lufs REAL NOT NULL,
    peak_dbfs     REAL NOT NULL,
    measured_s    REAL NOT NULL,
    updated_at    INTEGER NOT NULL
);
```

### `station_names`
The names the listener gives stations (version 5). Shown over the name stored with each history row, like and
session, which stays the name the station had then: the title parser relies on it.
```sql
CREATE TABLE IF NOT EXISTS station_names (
    station_id TEXT PRIMARY KEY,
    name       TEXT NOT NULL
);
```

### `settings`
Key/value settings (e.g. `click_counting`, `click_notice_seen`); every key, with its type and default, is in
`db/settings.rs`.
```sql
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

### `personal_stations`
Stations added by hand. Local, never sent anywhere, except when the listener shares one ([16-sharing.md](16-sharing.md)
§6.6). `origin` is `'link'` for a station received in a share link, whose stream stays off the local network
(16 §6.2), and `NULL` for one typed by hand (version 7).
```sql
CREATE TABLE IF NOT EXISTS personal_stations (
    station_id TEXT PRIMARY KEY DEFAULT ('user:' || lower(hex(randomblob(8)))),
    name       TEXT NOT NULL,
    stream_url TEXT NOT NULL UNIQUE,
    genre      TEXT,
    added_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
    origin     TEXT
);
```
