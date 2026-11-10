// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { hours, percent, plural, width } from './statsFormat';

describe('statsFormat', () => {
  it('says listening time in minutes, then hours', () => {
    expect(hours(0.5)).toBe('30 min');
    expect(hours(2.25)).toBe('2.3 h');
    expect(hours(12.4)).toBe('12 h');
  });

  it('gives small shares a decimal', () => {
    expect(percent(0.05)).toBe('5.0 %');
    expect(percent(0.42)).toBe('42 %');
  });

  it('puts the word in the plural except for one', () => {
    expect(plural(1, 'like')).toBe('1 like');
    expect(plural(0, 'like')).toBe('0 likes');
  });

  it('keeps a small bar visible and an empty chart flat', () => {
    expect(width(1, 1000)).toBe('2%');
    expect(width(5, 10)).toBe('50%');
    expect(width(3, 0)).toBe('0%');
  });
});
