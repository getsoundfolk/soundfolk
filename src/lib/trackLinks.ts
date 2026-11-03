// SPDX-License-Identifier: MPL-2.0
// Search pages where a track heard on air can be found again. Plain links: no account, no API.

export interface TrackService {
  name: string;
  url: (query: string) => string;
}

const q = encodeURIComponent;

export const TRACK_SERVICES: TrackService[] = [
  { name: 'YouTube', url: (t) => `https://www.youtube.com/results?search_query=${q(t)}` },
  { name: 'YouTube Music', url: (t) => `https://music.youtube.com/search?q=${q(t)}` },
  { name: 'Spotify', url: (t) => `https://open.spotify.com/search/${q(t)}` },
  { name: 'Deezer', url: (t) => `https://www.deezer.com/search/${q(t)}` },
  { name: 'Apple Music', url: (t) => `https://music.apple.com/search?term=${q(t)}` },
  { name: 'Bandcamp', url: (t) => `https://bandcamp.com/search?q=${q(t)}` },
  { name: 'Discogs', url: (t) => `https://www.discogs.com/search/?q=${q(t)}&type=all` },
];

/** Text to paste in a message: the track, the station it was heard on, and where. */
export const shareText = (title: string, station: string | null) =>
  station ? `${title} — heard on ${station} (via Soundfolk)` : `${title} (via Soundfolk)`;
