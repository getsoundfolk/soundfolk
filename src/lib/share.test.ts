// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { share, sharedOf } from './share.svelte';

const UUID = 'AB30CA84-C228-4F57-8CAC-816C69EF651D';

describe('sharedOf', () => {
  it('shares a catalog station by its UUID, in lower case', () => {
    expect(sharedOf({ id: UUID, name: 'FIP', url: 'https://x.org/fip', genre: null })).toEqual({
      kind: 'station',
      id: UUID.toLowerCase(),
    });
  });

  it('shares any other station by its name and stream', () => {
    const s = { id: 'user:3f2a', name: 'Mine', url: 'https://x.org/live', genre: null };
    expect(sharedOf(s)).toEqual({ kind: 'personal_station', name: 'Mine', url: s.url });
  });
});

describe('mayBeLink', () => {
  it('takes a web or soundfolk address, or a bare UUID, around spaces', () => {
    expect(share.mayBeLink('https://soundfolk.app/s/abc')).toBe(true);
    expect(share.mayBeLink('HTTP://example.org')).toBe(true);
    expect(share.mayBeLink('soundfolk://station/abc')).toBe(true);
    expect(share.mayBeLink(`  ${UUID}\n`)).toBe(true);
  });

  it('leaves a search alone', () => {
    expect(share.mayBeLink('fip jazz')).toBe(false);
    expect(share.mayBeLink('ftp://example.org')).toBe(false);
    expect(share.mayBeLink('see https://example.org')).toBe(false);
    expect(share.mayBeLink('https://example.org and more')).toBe(false);
    expect(share.mayBeLink('ab30ca84')).toBe(false);
    expect(share.mayBeLink('')).toBe(false);
  });
});
