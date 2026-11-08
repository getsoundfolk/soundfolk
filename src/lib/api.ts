// SPDX-License-Identifier: MPL-2.0
// Typed client for the Tauri commands. The types mirror the backend serde structures.
import { invoke } from '@tauri-apps/api/core';

export type PlayerState =
  | { status: 'idle' | 'connecting' | 'playing' | 'paused' }
  | { status: 'error' | 'reconnecting'; message: string };

export type TrackInfo = {
  artist: string | null;
  title: string;
  /** RFC 3339. */
  started_at: string | null;
  duration_secs: number | null;
  cover: string | null;
};

/** Where the title comes from: the stream itself, the station's web page, or the server's status. */
type TitleSource = 'icy' | 'stream_tags' | 'recipe' | 'server';

/** A newer version of the app (desktop only): its number and its release page. */
export type Release = { version: string; url: string };

/** Whether this copy checks for new versions (not a nightly or a packaged one), and if it is on. */
export type UpdateCheck = { supported: boolean; enabled: boolean };

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

/** Cover art of the track on air; `title` is the `NowPlaying.title` it belongs to. */
export type Cover = {
  title: string | null;
  url: string | null;
  /** `url` is the station's logo, shown when no source has the track's cover art. */
  logo?: boolean;
};

export type CoverSourceId = 'station' | 'deezer' | 'itunes' | 'cover_art_archive';
export type CoverSource = { id: CoverSourceId; enabled: boolean };

type Health =
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

interface SearchPage {
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

export interface HistorySummary {
  count: number;
  /** Oldest entry, SQLite UTC (`YYYY-MM-DD HH:MM:SS`), or null when empty. */
  since: string | null;
}

export interface ExportedFile {
  path: string;
  file_name: string;
  count: number;
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

/** A station's donation page (`soundfolk-stations`), and the domain the money goes to. */
export interface DonationLink {
  url: string;
  domain: string;
}

/** A curated list (`soundfolk-stations`, docs/15-curated-lists.md). */
export interface CuratedList {
  id: string;
  /** In the language its curator picked for its listeners, shown as is. */
  title: string;
  description: string;
  /** GitHub logins. */
  curators: string[];
  use: string[];
  genres: string[];
  /** ISO 3166-1 codes. */
  countries: string[];
  items: { uuid: string; stream?: string | null }[];
}

/** A list a station is in. */
export interface ListRef {
  id: string;
  title: string;
}

interface SearchQuery {
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
  pauseStream: () => invoke<void>('pause_stream'),
  resumeStream: () => invoke<void>('resume_stream'),
  setVolume: (volume: number) => invoke<void>('set_volume', { volume }),
  getMuted: () => invoke<boolean>('get_muted'),
  setMuted: (muted: boolean) => invoke<void>('set_muted', { muted }),
  getCurrentStation: () => invoke<Playable | null>('get_current_station'),
  getDonationLink: (stationId: string | null, url: string) =>
    invoke<DonationLink | null>('get_donation_link', { stationId, url }),
  getCuratedLists: () => invoke<CuratedList[]>('get_curated_lists'),
  getListsForStation: (stationId: string | null, url: string) =>
    invoke<ListRef[]>('get_lists_for_station', { stationId, url }),
  getCuratedListStations: (id: string) => invoke<Station[]>('get_curated_list_stations', { id }),
  exportCuratedList: (id: string) => invoke<ExportedFile>('export_curated_list', { id }),
  getFavoriteLists: () => invoke<string[]>('get_favorite_lists'),
  setFavoriteList: (id: string, favorite: boolean) =>
    invoke<string[]>('set_favorite_list', { id, favorite }),
  exportMyList: (list: 'favorites' | 'my_stations') =>
    invoke<ExportedFile>('export_my_list', { list }),
  getVolume: () => invoke<number | null>('get_volume'),
  saveVolume: (volume: number) => invoke<void>('save_volume', { volume }),
  getPlayerState: () => invoke<PlayerState>('get_player_state'),
  getNowPlaying: () => invoke<NowPlaying>('get_now_playing'),
  getCover: () => invoke<Cover>('get_cover'),
  getCoverSources: () => invoke<CoverSource[]>('get_cover_sources'),
  setCoverSources: (sources: CoverSource[]) =>
    invoke<CoverSource[]>('set_cover_sources', { sources }),

  searchStations: (query: SearchQuery) => invoke<SearchPage>('search_stations', { query }),
  /** A catalog station by its id (a Radio-Browser UUID); null when the catalog no longer has it. */
  getStation: (stationId: string) => invoke<Station | null>('get_station', { stationId }),
  /** Healthy music stations drawn at random, in the order to try them (Surprise me). */
  surpriseCandidates: (exclude: string | null) =>
    invoke<Station[]>('surprise_candidates', { exclude }),

  listFavorites: () => invoke<Favorite[]>('list_favorites'),
  addFavorite: (favorite: Favorite) => invoke<void>('add_favorite', { favorite }),
  removeFavorite: (stationId: string) => invoke<boolean>('remove_favorite', { stationId }),
  /** Saves the favorites' order (station ids, top first). */
  reorderFavorites: (stationIds: string[]) => invoke<void>('reorder_favorites', { stationIds }),

  /** `fromLink`: received in a share link, the stream stays off the local network (16 §6.2). */
  addPersonalStation: (url: string, name: string, fromLink = false) =>
    invoke<PersonalStation>('add_personal_station', { url, name, fromLink }),
  listPersonalStations: () => invoke<PersonalStation[]>('list_personal_stations'),
  removePersonalStation: (stationId: string) =>
    invoke<boolean>('remove_personal_station', { stationId }),

  /** Names the user gave stations, by station ID. */
  getStationNames: () => invoke<Record<string, string>>('get_station_names'),
  /** Names a station (null gives it back its own); returns the name kept. */
  setStationName: (stationId: string, name: string | null) =>
    invoke<string | null>('set_station_name', { stationId, name }),

  getListeningHistory: (limit?: number, query?: string) =>
    invoke<HistoryEntry[]>('get_listening_history', { limit, query }),
  getHistorySummary: () => invoke<HistorySummary>('get_history_summary'),
  clearListeningHistory: () => invoke<number>('clear_listening_history'),
  /** Writes the history or the liked tracks to the Downloads folder and shows the file. */
  exportHistory: (list: 'history' | 'likes', format: 'csv' | 'json', query?: string) =>
    invoke<ExportedFile>('export_history', { list, format, query }),

  likeTrack: (historyId: number) => invoke<boolean>('like_track', { historyId }),
  unlikeTrack: (trackKey: string) => invoke<boolean>('unlike_track', { trackKey }),
  listLikes: (limit?: number, query?: string) =>
    invoke<LikedTrack[]>('list_likes', { limit, query }),

  resolveStation: (stationId: string, name?: string) =>
    invoke<Playable>('resolve_station', { stationId, name }),

  getStats: (days?: number) => invoke<Stats>('get_stats', { days }),

  getTrackIdentification: () => invoke<boolean>('get_track_identification'),
  setTrackIdentification: (enabled: boolean) =>
    invoke<void>('set_track_identification', { enabled }),

  /** New versions (desktop only: these commands do not exist on mobile). */
  getUpdateCheck: () => invoke<UpdateCheck>('get_update_check'),
  setUpdateCheck: (enabled: boolean) => invoke<void>('set_update_check', { enabled }),
  getUpdate: () => invoke<Release | null>('get_update'),
  skipUpdate: (version: string) => invoke<void>('skip_update', { version }),

  getClickCounting: () => invoke<boolean>('get_click_counting'),
  setClickCounting: (enabled: boolean) => invoke<void>('set_click_counting', { enabled }),
  /** Launch at login, hidden in the notification area. */
  getAutostart: () => invoke<boolean>('get_autostart'),
  setAutostart: (enabled: boolean) => invoke<void>('set_autostart', { enabled }),
  /** A notification for each new track while the window is in the background. */
  getTrackNotifications: () => invoke<boolean>('get_track_notifications'),
  setTrackNotifications: (enabled: boolean) => invoke<void>('set_track_notifications', { enabled }),
  /** Constant volume: one gain per station so all play near the same loudness. */
  getConstantVolume: () => invoke<boolean>('get_constant_volume'),
  setConstantVolume: (enabled: boolean) => invoke<void>('set_constant_volume', { enabled }),
  /** Gain Constant volume applies to the station playing, in dB; null when off or idle. */
  getLevelAdjustment: () => invoke<number | null>('get_level_adjustment'),
  getWebTitles: () => invoke<boolean>('get_web_titles'),
  setWebTitles: (enabled: boolean) => invoke<void>('set_web_titles', { enabled }),
  getTitleStatus: () => invoke<TitleStatus>('get_title_status'),
  /** Picks up recipe files saved since the last check. */
  reloadRecipes: () => invoke<void>('reload_recipes'),
  testRecipe: (url: string, stationId?: string) =>
    invoke<RecipeTest>('test_recipe', { url, stationId }),
  editRecipe: (station: Playable) =>
    invoke<string>('edit_recipe', {
      url: station.url,
      stationId: station.id,
      name: station.name,
      homepage: station.homepage ?? null,
    }),
  /** Opens the "Share a recipe" form on GitHub, filled in with the recipe and its fixture. */
  shareRecipe: (url: string, stationId?: string) =>
    invoke<SharedRecipe>('share_recipe', { url, stationId }),
  getLocalRecipes: () => invoke<LocalRecipes>('get_local_recipes'),
  openRecipesFolder: () => invoke<void>('open_recipes_folder'),
  getTrackServices: () => invoke<string[] | null>('get_track_services'),
  setTrackServices: (ids: string[]) => invoke<void>('set_track_services', { ids }),
  getCustomTrackServices: () => invoke<CustomService[]>('get_custom_track_services'),
  setCustomTrackServices: (services: CustomService[]) =>
    invoke<void>('set_custom_track_services', { services }),

  /** Reads a pasted, dropped or opened text: null when it is not a Soundfolk link. */
  readShareLink: (text: string) => invoke<SharedLink | null>('read_share_link', { text }),
  /** The link for `item`, with the track on air and the sender's name, and its QR code. */
  makeShareLink: (item: Shared, track: string | null) =>
    invoke<ShareOut>('make_share_link', { item, track }),
  getShareName: () => invoke<string>('get_share_name'),
  setShareName: (name: string) => invoke<string>('set_share_name', { name }),
  /** The `soundfolk://` link opened from outside since the last call. */
  takeOpenedLink: () => invoke<string | null>('take_opened_link'),
  catalogStationForStream: (url: string) =>
    invoke<Station | null>('catalog_station_for_stream', { url }),
  /** Downloads the curated lists now (at most once a minute); true when new ones arrived. */
  refreshCuratedLists: () => invoke<boolean>('refresh_curated_lists'),

  /** Hands over from the main window to the mini player (the main window closes). */
  openMiniPlayer: () => invoke<void>('open_mini_player'),
  /** Back to the main window; the mini player closes. */
  closeMiniPlayer: () => invoke<void>('close_mini_player'),
  /** The mini player's full or compact size, kept for the next time. */
  setMiniCompact: (compact: boolean) => invoke<void>('set_mini_compact', { compact }),
  /** Next or previous favorite, as the media keys do it. */
  switchFavorite: (forward: boolean) => invoke<void>('switch_favorite', { forward }),
};

/** What a share link carries (docs/16-sharing.md §2.2). */
export type Shared =
  | { kind: 'station'; id: string }
  | { kind: 'personal_station'; name: string; url: string }
  | { kind: 'list'; id: string }
  | {
      kind: 'personal_list';
      title: string;
      stations: string[];
      personal: { name: string; url: string }[];
    };

/** A share link read: what it carries, the track on air then, and who sent it (unverified). */
export type SharedLink = Shared & { track: string | null; from: string | null };

/** A link ready to share. */
export interface ShareOut {
  url: string;
  /** What the link carries: a personal station found in the catalog became the catalog's. */
  item: Shared;
  /** Stream addresses of stations added by hand in the link. */
  addresses: number;
  /** Stations added by hand looked up in the catalog by their stream. */
  looked_up: number;
  /** SVG `data:` address, black on white. */
  qr: string | null;
  /** Modules on a side, quiet zone included. */
  qr_modules: number;
}

/** A site the user added to a track's menu: `{query}` in `template` stands for the track. */
export interface CustomService {
  id: string;
  name: string;
  template: string;
}

/** What the player can play, whatever the station's origin. */
export interface Playable {
  id: string;
  name: string;
  url: string;
  genre?: string | null;
  /** The station's website, when the catalog gives it. */
  homepage?: string | null;
}

/** A now-playing recipe (`<id>.toml`): shipped with the app, from the shared list, or in the user's folder. */
interface RecipeRef {
  id: string;
  name: string;
  origin: 'bundled' | 'shared' | 'local';
  path: string | null;
}

/** What "Share this recipe" did. */
export interface SharedRecipe {
  id: string;
  /** The recipe and fixture, laid out as in the repository. */
  folder: string;
  /** The fixture was too long for the link: it is in the clipboard. */
  fixture_in_clipboard: boolean;
}

/** The shared recipes repository. */
export const RECIPES_REPOSITORY = 'https://github.com/getsoundfolk/soundfolk-recipes';

/** The "Request a station" issue form, filled in with what is known about the station. */
export function recipeRequestLink(station: Playable): string {
  const catalog = station.id && !station.id.startsWith('user:') && station.id.length === 36;
  const params = new URLSearchParams({
    template: 'station.yml',
    title: `Station: ${station.name}`,
    station: catalog
      ? `${station.name} (https://www.radio-browser.info/history/${station.id})`
      : station.name,
    page: '',
    notes: [`Stream: ${station.url}`, station.homepage ? `Website: ${station.homepage}` : '']
      .filter(Boolean)
      .join('\n'),
  });
  return `${RECIPES_REPOSITORY}/issues/new?${params}`;
}

/** The station's local recipe file exists but cannot be used. */
interface LocalRecipeError {
  path: string;
  error: string;
}

/** Where the title of the current playback comes from. */
export interface TitleStatus {
  state: 'idle' | 'off' | 'searching' | 'stream' | 'recipe' | 'server' | 'none';
  recipe: RecipeRef | null;
  /** Streaming server status adapter (`icecast`, `shoutcast-v2`, `azuracast`…). */
  adapter: string | null;
  /** Why the last reading failed. */
  error: string | null;
  local_error: LocalRecipeError | null;
}

/** What a test reading of a recipe found. */
export interface RecipeTest {
  recipe: RecipeRef | null;
  local_error: LocalRecipeError | null;
  url: string | null;
  track: TrackInfo | null;
  next: TrackInfo | null;
  show: string | null;
  utc_offset_minutes: number | null;
  error: string | null;
}

/** Feedback after saving a local recipe. */
export interface RecipeNotice {
  file: string;
  ok: boolean;
  message: string;
}

export interface LocalRecipes {
  folder: string | null;
  count: number;
  errors: { file: string; error: string }[];
}

/** A favorite that Next / Previous skipped, because it did not broadcast. */
export interface Skipped {
  name: string;
  reason: string;
}

export interface StationChange extends Playable {
  show_click_notice: boolean;
  /** Favorites skipped on the way (none when the backend retries the same station). */
  skipped?: Skipped[];
}

/** What a failed command rejects with (`AppError` in error.rs). */
interface AppError {
  kind: 'database' | 'network' | 'catalog' | 'io' | 'other';
  message: string;
}

export const isAppError = (e: unknown): e is AppError =>
  typeof e === 'object' &&
  e !== null &&
  typeof (e as AppError).kind === 'string' &&
  typeof (e as AppError).message === 'string';

/** The text to show for anything a command or a promise rejected with. */
export const errorMessage = (e: unknown): string => {
  if (typeof e === 'string') return e;
  if (isAppError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
};
