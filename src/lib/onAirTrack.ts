// SPDX-License-Identifier: MPL-2.0
// How the player names the track on air (onAir.svelte.ts), apart so that it can be tested.
import type { NowPlaying } from './api';

/** Artist and title kept apart: by the source, or by the history. */
type Split = { artist: string | null; title: string };

/** The track on air, under the station's name: title and artist, and both on one line where there
 *  is room for one only.
 *
 *  Artist and title come apart from the source when it gives them that way, otherwise as the
 *  history split them (`logged`); a title the history did not log (a jingle) is shown whole. A
 *  title that only repeats the station's name is no track. */
export function trackOnAir(
  nowPlaying: Pick<NowPlaying, 'title' | 'track'>,
  logged: Split | null,
  stationName: string | null,
): { title: string | null; artist: string | null; track: string | null } {
  const split = nowPlaying.track ?? logged;
  const raw = split?.title ?? nowPlaying.title ?? null;
  const repeatsStation =
    !!raw && !!stationName && raw.trim().toLowerCase() === stationName.trim().toLowerCase();
  const title = nowPlaying.title && !repeatsStation ? raw : null;
  const artist = title ? (split?.artist ?? null) : null;
  const track = title && artist ? `${title} – ${artist}` : title;
  return { title, artist, track };
}
