# 💾 Local Database Specification

## 1. Data strategy
* **Catalog:** the global station catalog comes from **Radio-Browser** and is queried online through `CatalogSource`.
  There is no full local mirror: without a network no radio can be played, so offline catalog search has no value.
* **Local persistence (SQLite):** the user's favorites, listening history, personal stations and settings, with 0 ms
  responsiveness.
* **Station IDs:** `station_id` (favorites, history, reports) is the Radio-Browser UUID for a catalog station, or
  `user:<hex>` for a personal station.

## 2. Local tables (`rusqlite`, WAL mode)

### `favorites`
Stations pinned by the user.
```sql
CREATE TABLE IF NOT EXISTS favorites (
    station_id TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    stream_url TEXT NOT NULL,
    genre      TEXT,
    added_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### `listening_history`
Tracks played on air, recorded from the stream's ICY metadata (consecutive duplicates are skipped).
`station_name` is copied at listening time, because catalog stations are not stored locally: without it, a catalog
UUID could not be shown by name. Rows from before this column have it `NULL`; the app then falls back to the favorite
or personal station of the same ID, if any.

`track_title` is the raw title and is never altered. `artist`, `title` and `track_key` are parsed from it (`tracks.rs`:
station signature removed, then `Artist - Title` or `Title by Artist`). When the parsing rules change, the
`title_parser_version` setting no longer matches and every row is re-parsed at startup. `track_key` ignores case,
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

### `station_reports`
Anomaly reports about catalog stations.
```sql
CREATE TABLE IF NOT EXISTS station_reports (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    station_id  TEXT NOT NULL,
    report_type TEXT NOT NULL CHECK (report_type IN ('dead_link', 'duplicate', 'outdated_info')),
    comment     TEXT,
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### `settings`
Key/value settings (e.g. `click_counting`, `click_notice_seen`).
```sql
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

### `personal_stations`
Stations added by hand. Strictly local, never sent anywhere.
```sql
CREATE TABLE IF NOT EXISTS personal_stations (
    station_id TEXT PRIMARY KEY DEFAULT ('user:' || lower(hex(randomblob(8)))),
    name       TEXT NOT NULL,
    stream_url TEXT NOT NULL UNIQUE,
    genre      TEXT,
    added_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
```
