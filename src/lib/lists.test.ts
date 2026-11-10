// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import type { CuratedList } from './api';
import { lists } from './lists.svelte';

const list = (id: string, title: string, description: string, curators: string[]) =>
  ({ id, title, description, curators, items: [] }) satisfies CuratedList;

describe('lists.matching', () => {
  lists.all = [
    list('a', 'Jazz Clubs', 'Late nights in Paris', ['Ann Lee']),
    list('b', 'Morning Jazz', 'Coffee and piano', ['Bob']),
    list('c', 'Électro', 'Beats from Lyon', ['Ann Lee']),
  ];
  const ids = (text: string) => lists.matching(text).map((l) => l.id);

  it('finds a word in the title, the description or a curator, whatever the case', () => {
    expect(ids('jazz')).toEqual(['a', 'b']);
    expect(ids('PIANO')).toEqual(['b']);
    expect(ids('ann')).toEqual(['a', 'c']);
    expect(ids('électro')).toEqual(['c']);
  });

  it('needs every word, in any field and any order', () => {
    expect(ids('  paris   jazz ')).toEqual(['a']);
    expect(ids('ann lyon')).toEqual(['c']);
    expect(ids('jazz lyon')).toEqual([]);
  });

  it('finds nothing for an empty search', () => {
    expect(ids('')).toEqual([]);
    expect(ids('   ')).toEqual([]);
  });
});
