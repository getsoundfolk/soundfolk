// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { hostOf, pickRandom } from './stations';

describe('pickRandom', () => {
  it('never picks the excluded one while there are others', () => {
    for (let i = 0; i < 50; i++) expect(pickRandom([1, 2, 3], (n) => n === 2)).not.toBe(2);
  });

  it('picks the excluded one when it is the only one, and nothing from nothing', () => {
    expect(pickRandom([2], (n) => n === 2)).toBe(2);
    expect(pickRandom([], () => false)).toBeNull();
  });
});

describe('hostOf', () => {
  it('reads the host, or gives the text back', () => {
    expect(hostOf('https://icecast.radiofrance.fr/fip-midfi.mp3')).toBe('icecast.radiofrance.fr');
    expect(hostOf('http://51.75.1.2:8000/live')).toBe('51.75.1.2:8000');
    expect(hostOf('not an address')).toBe('not an address');
  });
});
