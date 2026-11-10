// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { RECIPES_REPOSITORY, recipeRequestLink } from './api';

const params = (link: string) => {
  const url = new URL(link);
  expect(`${url.origin}${url.pathname}`).toBe(`${RECIPES_REPOSITORY}/issues/new`);
  return url.searchParams;
};

describe('recipeRequestLink', () => {
  it('fills in a catalog station with its Radio-Browser page', () => {
    const id = 'ab30ca84-c228-4f57-8cac-816c69ef651d';
    const p = params(
      recipeRequestLink({
        id,
        name: 'FIP & Co',
        url: 'https://x.org/fip?a=1',
        genre: null,
        homepage: 'https://fip.fr',
      }),
    );
    expect(p.get('template')).toBe('station.yml');
    expect(p.get('title')).toBe('Station: FIP & Co');
    expect(p.get('station')).toBe(`FIP & Co (https://www.radio-browser.info/history/${id})`);
    expect(p.get('page')).toBe('');
    expect(p.get('notes')).toBe('Stream: https://x.org/fip?a=1\nWebsite: https://fip.fr');
  });

  it('names any other station, without a website it does not know', () => {
    const p = params(
      recipeRequestLink({ id: 'user:3f2a', name: 'Mine', url: 'https://x.org/live', genre: null }),
    );
    expect(p.get('station')).toBe('Mine');
    expect(p.get('notes')).toBe('Stream: https://x.org/live');
  });
});
