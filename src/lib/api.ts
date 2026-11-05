// SPDX-License-Identifier: MPL-2.0
// Typed client for the Tauri commands. The types mirror the backend serde structures.
import { invoke } from '@tauri-apps/api/core';

export type PlayerState =
  | { status: 'idle' | 'connecting' | 'playing' | 'paused' }
  | { status: 'error'; message: string };

export type TrackInfo = {
  artist: string | null;
  title: string;
  /** RFC 3339. */
  started_at: string | null;
  duration_secs: number | null;
  cover: string | null;
};

/** Where the title comes from: the stream itself, the station's web page, or the server's status. */
export type TitleSource = 'icy' | 'stream_tags' | 'recipe' | 'server';

export type NowPlaying = {
  station: string | null;
  /** `Artist - Title` as shown. */
  title: string | null;
  /** Artist and title apart, when the source gives them that way. */
  track?: TrackInfo | null;
  next?: TrackInfo | null;
  show?: string | null;
  source?: TitleSource | null;
};

export type Health =
  | { status: 'ok' }
  | { status: 'unreachable'; last_ok: string | null }
  | { status: 'unsupported'; reason: string };

export interface Station {
  id: string;
  name: string;
  url: string;
  homepage: string | null;
  favicon: string | null;
  tags: string[];
  country: string | null;
  country_code: string | null;
  language: string | null;
  codec: string | null;
  bitrate: number;
  votes: number;
  health: Health;
}

export interface SearchPage {
  stations: Station[];
  /** Offset of the next page, or `null` when there are no more results. */
  next_offset: number | null;
}

export interface PersonalStation {
  station_id: string;
  name: string;
  stream_url: string;
  genre: string | null;
}

export interface Favorite {
  station_id: string;
  name: string;
  stream_url: string;
  genre: string | null;
}

export interface HistoryEntry {
  id: number;
  station_id: string;
  station_name: string | null;
  /** Raw title as announced on air. */
  track_title: string;
  artist: string | null;
  title: string;
  track_key: string;
  /** SQLite `CURRENT_TIMESTAMP`: UTC, `YYYY-MM-DD HH:MM:SS`. */
  played_at: string;
  liked: boolean;
  /** Artist and title cleaned for a music service's search box. */
  search_query: string;
}

export interface LikedTrack {
  track_key: string;
  artist: string | null;
  title: string;
  raw_title: string;
  /** Station the track was liked on. */
  station_id: string;
  station_name: string | null;
  liked_at: string;
  search_query: string;
}

export interface Count {
  label: string;
  detail: string | null;
  count: number;
}

export interface StationStats {
  station_id: string;
  name: string;
  likes: number;
  tracks_heard: number;
  listening_hours: number;
  likes_per_hour: number | null;
  like_rate: number | null;
  repetition: number | null;
  diversity: number | null;
  unrecognised_share: number | null;
}

export interface Stats {
  totals: { tracks_heard: number; likes: number; listening_hours: number };
  identification: { enabled: boolean; matched: number; not_found: number; pending: number };
  stations: StationStats[];
  artists: Count[];
  albums: Count[];
  discovery_stations: Count[];
  /** `YYYY-MM`, oldest first. */
  new_artists_by_month: Count[];
  shared_tracks: Count[];
  /** Monday first. */
  hours_by_weekday: number[];
  hours_by_hour: number[];
  decades: Count[];
  genres: Count[];
  /** ISO 3166-1 codes. */
  countries: Count[];
}

export interface SearchQuery {
  text?: string;
  country_code?: string;
  tag?: string;
  limit?: number;
  offset?: number;
}

export const api = {
  playStream: (url: string, stationId?: string, stationName?: string) =>
    invoke<{ show_click_notice: boolean }>('play_stream', { url, stationId, stationName }),
  stopStream: () => invoke<void>('stop_stream'),
  setVolume: (volume: number) => invoke<void>('set_volume', { volume }),
  getPlayerState: () => invoke<PlayerState>('get_player_state'),
  getNowPlaying: () => invoke<NowPlaying>('get_now_playing'),

  searchStations: (query: SearchQuery) => invoke<SearchPage>('search_stations', { query }),

  listFavorites: () => invoke<Favorite[]>('list_favorites'),
  addFavorite: (favorite: Favorite) => invoke<void>('add_favorite', { favorite }),
  removeFavorite: (stationId: string) => invoke<boolean>('remove_favorite', { stationId }),

  addPersonalStation: (url: string, name?: string) =>
    invoke<PersonalStation>('add_personal_station', { url, name }),
  listPersonalStations: () => invoke<PersonalStation[]>('list_personal_stations'),
  removePersonalStation: (stationId: string) =>
    invoke<boolean>('remove_personal_station', { stationId }),

  getListeningHistory: (limit?: number, query?: string) =>
    invoke<HistoryEntry[]>('get_listening_history', { limit, query }),
  clearListeningHistory: () => invoke<number>('clear_listening_history'),

  likeTrack: (historyId: number) => invoke<boolean>('like_track', { historyId }),
  unlikeTrack: (trackKey: string) => invoke<boolean>('unlike_track', { trackKey }),
  listLikes: (limit?: number, query?: string) => invoke<LikedTrack[]>('list_likes', { limit, query }),

  resolveStation: (stationId: string, name?: string) =>
    invoke<Playable>('resolve_station', { stationId, name }),

  getStats: (days?: number) => invoke<Stats>('get_stats', { days }),

  getTrackIdentification: () => invoke<boolean>('get_track_identification'),
  setTrackIdentification: (enabled: boolean) => invoke<void>('set_track_identification', { enabled }),

  getClickCounting: () => invoke<boolean>('get_click_counting'),
  setClickCounting: (enabled: boolean) => invoke<void>('set_click_counting', { enabled }),
  getWebTitles: () => invoke<boolean>('get_web_titles'),
  setWebTitles: (enabled: boolean) => invoke<void>('set_web_titles', { enabled }),
};

/** What the player can play, whatever the station's origin. */
export interface Playable {
  id: string;
  name: string;
  url: string;
  genre?: string | null;
}

export interface StationChange extends Playable {
  show_click_notice: boolean;
}

export const errorMessage = (e: unknown): string => (typeof e === 'string' ? e : String(e));
