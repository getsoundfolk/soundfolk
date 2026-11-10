// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { target } from './a11y';

describe('target', () => {
  it('moves along its own axis and wraps around', () => {
    expect(target('ArrowRight', 0, 3, false)).toBe(1);
    expect(target('ArrowRight', 2, 3, false)).toBe(0);
    expect(target('ArrowLeft', 0, 3, false)).toBe(2);
    expect(target('ArrowDown', 2, 3, true)).toBe(0);
    expect(target('ArrowUp', 0, 3, true)).toBe(2);
  });

  it('ignores the other axis and other keys', () => {
    expect(target('ArrowDown', 0, 3, false)).toBeNull();
    expect(target('ArrowRight', 0, 3, true)).toBeNull();
    expect(target('Enter', 0, 3, true)).toBeNull();
  });

  it('goes to the ends with Home and End', () => {
    expect(target('Home', 2, 3, false)).toBe(0);
    expect(target('End', 0, 3, true)).toBe(2);
  });

  it('stays on a single item', () => {
    expect(target('ArrowDown', 0, 1, true)).toBe(0);
    expect(target('ArrowUp', 0, 1, true)).toBe(0);
  });
});
