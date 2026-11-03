// SPDX-License-Identifier: MPL-2.0
// Typed client for the Tauri commands. The types mirror the backend serde structures.
import { invoke } from '@tauri-apps/api/core';

export type PlayerState =
  | { status: 'idle' | 'connecting' | 'playing' | 'paused' }
  | { status: 'error'; message: string };

export type NowPlaying = { station: string | null; title: string | null };

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

  getClickCounting: () => invoke<boolean>('get_click_counting'),
  setClickCounting: (enabled: boolean) => invoke<void>('set_click_counting', { enabled }),
};

/** What the player can play, whatever the station's origin. */
export interface Playable {
  id: string;
  name: string;
  url: string;
  genre?: string | null;
}

export const errorMessage = (e: unknown): string => (typeof e === 'string' ? e : String(e));
