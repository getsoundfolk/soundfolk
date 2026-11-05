// SPDX-License-Identifier: MPL-2.0
// Search pages where a track heard on air can be found again. Plain links: no account, no API.

import type { CustomService } from './api';

export type ServiceGroup = 'listen' | 'buy' | 'learn' | 'custom';

export interface TrackService {
  /** Stable id, stored in the settings: never rename one. */
  id: string;
  name: string;
  group: ServiceGroup;
  url: (query: string) => string;
}

export const SERVICE_GROUPS: { id: ServiceGroup; label: string; detail: string }[] = [
  { id: 'listen', label: 'Listen', detail: 'Streaming services and video' },
  { id: 'buy', label: 'Buy', detail: 'Downloads without DRM, paid to the artist or label' },
  {
    id: 'learn',
    label: 'Learn more',
    detail: 'Credits, pressings, lyrics, samples, reviews, concerts',
  },
];

const q = encodeURIComponent;

/**
 * Only services that take a plain text search at a fixed address. Left out on purpose: services
 * without one central site (Faircamp, Funkwhale, PeerTube, Invidious, Piped), link converters that
 * need a platform link rather than a title (Songlink/Odesli), and sites searched by album only.
 */
export const TRACK_SERVICES: TrackService[] = [
  {
    id: 'youtube',
    name: 'YouTube',
    group: 'listen',
    url: (t) => `https://www.youtube.com/results?search_query=${q(t)}`,
  },
  {
    id: 'youtube-music',
    name: 'YouTube Music',
    group: 'listen',
    url: (t) => `https://music.youtube.com/search?q=${q(t)}`,
  },
  {
    id: 'spotify',
    name: 'Spotify',
    group: 'listen',
    url: (t) => `https://open.spotify.com/search/${q(t)}`,
  },
  {
    id: 'deezer',
    name: 'Deezer',
    group: 'listen',
    url: (t) => `https://www.deezer.com/search/${q(t)}`,
  },
  {
    id: 'apple-music',
    name: 'Apple Music',
    group: 'listen',
    url: (t) => `https://music.apple.com/search?term=${q(t)}`,
  },
  {
    id: 'tidal',
    name: 'Tidal',
    group: 'listen',
    url: (t) => `https://listen.tidal.com/search?q=${q(t)}`,
  },
  {
    id: 'qobuz',
    name: 'Qobuz',
    group: 'listen',
    url: (t) => `https://play.qobuz.com/search?q=${q(t)}`,
  },
  {
    id: 'amazon-music',
    name: 'Amazon Music',
    group: 'listen',
    url: (t) => `https://music.amazon.com/search/${q(t)}`,
  },
  {
    id: 'soundcloud',
    name: 'SoundCloud',
    group: 'listen',
    url: (t) => `https://soundcloud.com/search?q=${q(t)}`,
  },
  {
    id: 'mixcloud',
    name: 'Mixcloud',
    group: 'listen',
    url: (t) => `https://www.mixcloud.com/search/?q=${q(t)}`,
  },

  {
    id: 'bandcamp',
    name: 'Bandcamp',
    group: 'buy',
    url: (t) => `https://bandcamp.com/search?q=${q(t)}`,
  },
  {
    id: 'beatport',
    name: 'Beatport',
    group: 'buy',
    url: (t) => `https://www.beatport.com/search?q=${q(t)}`,
  },
  {
    id: 'traxsource',
    name: 'Traxsource',
    group: 'buy',
    url: (t) => `https://www.traxsource.com/search?term=${q(t)}`,
  },

  {
    id: 'discogs',
    name: 'Discogs',
    group: 'learn',
    url: (t) => `https://www.discogs.com/search/?q=${q(t)}&type=all`,
  },
  {
    id: 'musicbrainz',
    name: 'MusicBrainz',
    group: 'learn',
    url: (t) => `https://musicbrainz.org/search?query=${q(t)}&type=recording&method=indexed`,
  },
  {
    id: 'lastfm',
    name: 'Last.fm',
    group: 'learn',
    url: (t) => `https://www.last.fm/search?q=${q(t)}`,
  },
  {
    id: 'genius',
    name: 'Genius',
    group: 'learn',
    url: (t) => `https://genius.com/search?q=${q(t)}`,
  },
  {
    id: 'whosampled',
    name: 'WhoSampled',
    group: 'learn',
    url: (t) => `https://www.whosampled.com/search/?q=${q(t)}`,
  },
  {
    id: 'rateyourmusic',
    name: 'Rate Your Music',
    group: 'learn',
    url: (t) => `https://rateyourmusic.com/search?searchterm=${q(t)}&searchtype=z`,
  },
  {
    id: 'allmusic',
    name: 'AllMusic',
    group: 'learn',
    url: (t) => `https://www.allmusic.com/search/all/${q(t)}`,
  },
  {
    id: 'setlistfm',
    name: 'setlist.fm',
    group: 'learn',
    url: (t) => `https://www.setlist.fm/search?query=${q(t)}`,
  },
];

/** Shown until the user picks their own: the menu as it has always been. */
export const DEFAULT_SERVICE_IDS = [
  'youtube',
  'youtube-music',
  'spotify',
  'deezer',
  'apple-music',
  'bandcamp',
  'discogs',
];

export const MAX_CUSTOM = 10;
const MAX_NAME = 40;
const PLACEHOLDER = '{query}';

export const customToService = (c: CustomService): TrackService => ({
  id: c.id,
  name: c.name,
  group: 'custom',
  url: (t) => c.template.replace(PLACEHOLDER, q(t)),
});

/**
 * Same checks as the backend (`track_services.rs`), to explain a problem while typing.
 * Returns what is wrong, or null.
 */
export function checkCustom(name: string, template: string): string | null {
  const n = name.trim();
  const t = template.trim();
  if (!n) return 'Give the site a name';
  if ([...n].length > MAX_NAME) return `The name must be at most ${MAX_NAME} characters`;
  if (!t.startsWith('https://')) return 'The address must start with https://';
  if (t.split(PLACEHOLDER).length !== 2) return 'Put {query} once where the search goes';
  if (t.slice('https://'.length).startsWith('/'))
    return 'The address needs a site name, without login';
  try {
    const url = new URL(t.replace(PLACEHOLDER, 'test'));
    if (!url.hostname || url.username || url.password)
      return 'The address needs a site name, without login';
  } catch {
    return 'This address is not valid';
  }
  return null;
}

/** `Artist - Title`, or the title alone. */
export const displayTitle = (t: { artist: string | null; title: string }) =>
  t.artist ? `${t.artist} - ${t.title}` : t.title;

/** Text to paste in a message: the track, the station it was heard on, and where. */
export const shareText = (track: string, station: string | null) =>
  station ? `${track} — heard on ${station} (via Soundfolk)` : `${track} (via Soundfolk)`;
