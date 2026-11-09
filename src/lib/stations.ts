// SPDX-License-Identifier: MPL-2.0
// Catalog stations as the rows and the player show them.
import type { Favorite, PersonalStation, Playable, Station } from './api';

export const playableOf = (s: Station): Playable => ({
  id: s.id,
  name: s.name,
  url: s.url,
  genre: s.tags[0] ?? null,
  homepage: s.homepage,
});

/** A favorite or a personal station, as the player plays it. */
export const playableOfSaved = (s: Favorite | PersonalStation): Playable => ({
  id: s.station_id,
  name: s.name,
  url: s.stream_url,
  genre: s.genre,
});

/** What a list row says of a catalog station besides its name: its country. Genres come once
 *  Radio-Browser's tags map to translated genres (06 §7: raw tags are never shown); codec and
 *  bitrate belong to the station sheet (06 §5). */
export const shortMeta = (s: Station) => s.country_code ?? '';

/** The host of a stream address, or the address itself when it cannot be read. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** What a row and the station sheet say of a stream the catalog could not reach; null otherwise. */
export function warningOf(s: Station): string | null {
  if (s.health.status !== 'unreachable') return null;
  const last = s.health.last_ok ? new Date(s.health.last_ok) : null;
  return last && !Number.isNaN(last.getTime())
    ? `Unreachable at last check (last success: ${last.toLocaleDateString(undefined, { dateStyle: 'medium' })})`
    : 'Unreachable at last check';
}

/** One of `pool` at random, never `exclude` unless it is the only one; null when `pool` is empty. */
export function pickRandom<T>(pool: T[], exclude: (t: T) => boolean): T | null {
  const others = pool.filter((t) => !exclude(t));
  const from = others.length ? others : pool;
  return from[Math.floor(Math.random() * from.length)] ?? null;
}
