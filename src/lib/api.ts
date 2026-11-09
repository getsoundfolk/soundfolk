// SPDX-License-Identifier: MPL-2.0
// The Tauri commands and the types they carry come from the backend: `bindings.ts` is generated
// from the Rust code (src-tauri/src/bindings.rs), never written by hand. This module adds what
// only the front end needs.
import { listen as listenTauri, type EventCallback, type UnlistenFn } from '@tauri-apps/api/event';
import { commands, type AppError, type EventPayloads, type ResolvedStation } from './bindings';

export { commands as api };
export type {
  Cover,
  CoverSource,
  Count,
  CuratedList,
  CustomService,
  DonationLink,
  Favorite,
  HistoryEntry,
  HistorySummary,
  LikedTrack,
  ListRef,
  LocalRecipes,
  NowPlaying,
  PersonalStation,
  PlayerState,
  RecipeNotice,
  RecipeTest,
  Release,
  Shared,
  SharedLink,
  SharedRecipe,
  ShareOut,
  Skipped,
  SourceId as CoverSourceId,
  Station,
  Stats,
  StationStats,
  TitleStatus,
} from './bindings';

/** Listens to an event the backend sends, its payload typed by its name (`EventPayloads`). */
export const listen = <K extends keyof EventPayloads>(
  event: K,
  handler: EventCallback<EventPayloads[K]>,
): Promise<UnlistenFn> => listenTauri(event, handler);

/** What the player can play, whatever the station's origin. */
export type Playable = Omit<ResolvedStation, 'genre'> & {
  genre?: string | null;
  /** The station's website, when the catalog gives it. */
  homepage?: string | null;
};

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
