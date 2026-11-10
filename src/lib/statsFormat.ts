// SPDX-License-Identifier: MPL-2.0
// Figures of the Stats panel, in words and bar sizes.

/** Listening time: minutes under an hour, one decimal under ten hours. */
export const hours = (h: number) =>
  h < 1 ? `${Math.round(h * 60)} min` : `${h.toFixed(h < 10 ? 1 : 0)} h`;

/** A share: one decimal under 10 %. */
export const percent = (r: number) => `${(r * 100).toFixed(r < 0.1 ? 1 : 0)} %`;

export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** A bar's width against the longest: never under 2 %, so a small value still shows. */
export const width = (value: number, top: number) =>
  `${top > 0 ? Math.max((value / top) * 100, 2) : 0}%`;
