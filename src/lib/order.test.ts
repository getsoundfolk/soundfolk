// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { moved } from './order';

describe('moved', () => {
  it('moves an item down and up, leaving the list untouched', () => {
    const list = ['a', 'b', 'c', 'd'];
    expect(moved(list, 0, 2)).toEqual(['b', 'c', 'a', 'd']);
    expect(moved(list, 3, 0)).toEqual(['d', 'a', 'b', 'c']);
    expect(moved(list, 1, 1)).toEqual(list);
    expect(list).toEqual(['a', 'b', 'c', 'd']);
  });
});
