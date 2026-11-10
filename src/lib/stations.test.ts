// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { hostOf, isCatalogId, pickRandom, quality } from './stations';

describe('pickRandom', () => {
  it('never picks the excluded one while there are others', () => {
    for (let i = 0; i < 50; i++) expect(pickRandom([1, 2, 3], (n) => n === 2)).not.toBe(2);
  });

  it('picks the excluded one when it is the only one, and nothing from nothing', () => {
    expect(pickRandom([2], (n) => n === 2)).toBe(2);
    expect(pickRandom([], () => false)).toBeNull();
  });
});

describe('isCatalogId', () => {
  it('takes only a Radio-Browser UUID', () => {
    expect(isCatalogId('ab30ca84-c228-4f57-8cac-816c69ef651d')).toBe(true);
    expect(isCatalogId('AB30CA84-C228-4F57-8CAC-816C69EF651D')).toBe(true);
    expect(isCatalogId('user:3f2a')).toBe(false);
    // A stream address of 36 characters is not a UUID.
    expect(isCatalogId('http://radio.example/stream-128k.mp3')).toBe(false);
    expect(isCatalogId('')).toBe(false);
    expect(isCatalogId(null)).toBe(false);
  });
});

describe('hostOf', () => {
  it('reads the host, or gives the text back', () => {
    expect(hostOf('https://icecast.radiofrance.fr/fip-midfi.mp3')).toBe('icecast.radiofrance.fr');
    expect(hostOf('http://51.75.1.2:8000/live')).toBe('51.75.1.2:8000');
    expect(hostOf('not an address')).toBe('not an address');
  });
});

describe('quality', () => {
  it('says the quality in words, then the figures', () => {
    expect(quality({ codec: 'MP3', bitrate: 128 })).toBe('Good quality — MP3 128 kbit/s');
    expect(quality({ codec: 'MP3', bitrate: 320 })).toBe('High quality — MP3 320 kbit/s');
    expect(quality({ codec: 'MP3', bitrate: 64 })).toBe('Low quality — MP3 64 kbit/s');
  });

  it('counts AAC, Opus and Vorbis at twice their bitrate', () => {
    expect(quality({ codec: 'AAC+', bitrate: 64 })).toBe('Good quality — AAC+ 64 kbit/s');
    expect(quality({ codec: 'OGG', bitrate: 96 })).toBe('High quality — OGG 96 kbit/s');
    expect(quality({ codec: 'opus', bitrate: 48 })).toBe('Low quality — opus 48 kbit/s');
  });

  it('calls FLAC lossless whatever the bitrate', () => {
    expect(quality({ codec: 'FLAC', bitrate: 0 })).toBe('Lossless — FLAC');
    expect(quality({ codec: 'flac', bitrate: 900 })).toBe('Lossless — flac 900 kbit/s');
  });

  it('says what it knows, and nothing without codec or bitrate', () => {
    expect(quality({ codec: ' MP3 ', bitrate: 0 })).toBe('MP3');
    expect(quality({ codec: null, bitrate: 128 })).toBe('Good quality — 128 kbit/s');
    expect(quality({ codec: '  ', bitrate: 0 })).toBeNull();
    expect(quality({ codec: null, bitrate: 0 })).toBeNull();
  });
});
