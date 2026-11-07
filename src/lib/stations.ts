// SPDX-License-Identifier: MPL-2.0
// Catalog stations as the rows and the player show them.
import type { Playable, Station } from './api';

export const playableOf = (s: Station): Playable => ({
  id: s.id,
  name: s.name,
  url: s.url,
  genre: s.tags[0] ?? null,
  homepage: s.homepage,
});

/** Country and the first two tags, the short form used in the lists. */
export const shortMeta = (s: Station) =>
  [s.country_code, s.tags.slice(0, 2).join(', ')].filter(Boolean).join(' · ');

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
