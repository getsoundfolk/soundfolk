// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import type { TrackInfo } from './bindings';
import { trackOnAir } from './onAirTrack';

const info = (artist: string | null, title: string): TrackInfo => ({
  artist,
  title,
  started_at: null,
  duration_secs: null,
  cover: null,
});

describe('trackOnAir', () => {
  it('takes artist and title from the source first', () => {
    const now = { title: 'Nina Simone - Sinnerman', track: info('Nina Simone', 'Sinnerman') };
    expect(trackOnAir(now, { artist: 'Other', title: 'Other' }, 'FIP')).toEqual({
      title: 'Sinnerman',
      artist: 'Nina Simone',
      track: 'Sinnerman – Nina Simone',
    });
  });

  it('then as the history split them', () => {
    const now = { title: 'Nina Simone - Sinnerman', track: null };
    expect(trackOnAir(now, { artist: 'Nina Simone', title: 'Sinnerman' }, 'FIP')).toEqual({
      title: 'Sinnerman',
      artist: 'Nina Simone',
      track: 'Sinnerman – Nina Simone',
    });
  });

  it('shows a title the history did not log whole, without an artist', () => {
    expect(trackOnAir({ title: 'Jingle FIP', track: null }, null, 'FIP')).toEqual({
      title: 'Jingle FIP',
      artist: null,
      track: 'Jingle FIP',
    });
  });

  it('shows no track for a title that only repeats the station', () => {
    const none = { title: null, artist: null, track: null };
    expect(trackOnAir({ title: ' fip ', track: null }, null, 'FIP')).toEqual(none);
    expect(trackOnAir({ title: 'x', track: info(null, 'Radio Nova') }, null, 'radio nova')).toEqual(
      none,
    );
  });

  it('shows nothing while nothing is on air', () => {
    const none = { title: null, artist: null, track: null };
    expect(trackOnAir({ title: null, track: null }, null, 'FIP')).toEqual(none);
    // A split left from before the title was cleared does not bring it back.
    expect(trackOnAir({ title: null, track: null }, { artist: 'A', title: 'T' }, null)).toEqual(
      none,
    );
  });
});
