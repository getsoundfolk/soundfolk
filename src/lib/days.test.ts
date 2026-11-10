// SPDX-License-Identifier: MPL-2.0
import { describe, expect, it } from 'vitest';
import { dayLabel, utc } from './days';

describe('utc', () => {
  it('reads a SQLite date as UTC', () => {
    expect(utc('2026-11-10 08:30:05').toISOString()).toBe('2026-11-10T08:30:05.000Z');
  });

  it('gives an invalid date for something else', () => {
    expect(Number.isNaN(utc('yesterday').getTime())).toBe(true);
  });
});

describe('dayLabel', () => {
  const today = new Date(2026, 10, 10, 9, 0);

  it('names today and yesterday, from midnight to midnight local time', () => {
    expect(dayLabel(new Date(2026, 10, 10, 0, 0), today)).toBe('Today');
    expect(dayLabel(new Date(2026, 10, 10, 23, 59), today)).toBe('Today');
    expect(dayLabel(new Date(2026, 10, 9, 23, 59), today)).toBe('Yesterday');
    expect(dayLabel(new Date(2026, 10, 9, 0, 0), today)).toBe('Yesterday');
  });

  it('dates the days before, also across a month and a year', () => {
    const older = new Date(2026, 10, 8, 12, 0);
    const label = older.toLocaleDateString(undefined, {
      weekday: 'long',
      day: 'numeric',
      month: 'long',
    });
    expect(dayLabel(older, today)).toBe(label);
    expect(dayLabel(new Date(2026, 9, 31, 12, 0), new Date(2026, 10, 1, 8, 0))).toBe('Yesterday');
    expect(dayLabel(new Date(2025, 11, 31, 12, 0), new Date(2026, 0, 1, 8, 0))).toBe('Yesterday');
  });
});
