// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { checkCustom, displayTitle } from './trackLinks';

describe('checkCustom', () => {
  it('accepts a search address with one {query}', () => {
    expect(checkCustom('Qobuz', 'https://www.qobuz.com/search?q={query}')).toBeNull();
  });

  it('says what is wrong', () => {
    expect(checkCustom(' ', 'https://example.org/?q={query}')).toBe('Give the site a name');
    expect(checkCustom('Site', 'http://example.org/?q={query}')).toMatch(/https:\/\//);
    expect(checkCustom('Site', 'https://example.org/')).toMatch(/\{query\} once/);
    expect(checkCustom('Site', 'https://example.org/{query}/{query}')).toMatch(/\{query\} once/);
    expect(checkCustom('Site', 'https:///search?q={query}')).toMatch(/without login/);
    expect(checkCustom('Site', 'https://me:secret@example.org/?q={query}')).toMatch(
      /without login/,
    );
  });
});

describe('displayTitle', () => {
  it('joins as the backend does', () => {
    expect(displayTitle({ artist: 'Air', title: 'Sexy Boy' })).toBe('Air - Sexy Boy');
    expect(displayTitle({ artist: null, title: 'Jingle' })).toBe('Jingle');
  });
});
