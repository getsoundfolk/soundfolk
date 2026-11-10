// SPDX-License-Identifier: MPL-2.0
// Dates as the history shows them.

/** A date SQLite stored (UTC, without a zone marker: `2026-11-10 08:30:00`). */
export const utc = (sqlite: string) => new Date(`${sqlite.replace(' ', 'T')}Z`);

/** The heading of a day in the history: Today, Yesterday, or the date in the local language. */
export function dayLabel(date: Date, today: Date): string {
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return 'Today';
  if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
}
