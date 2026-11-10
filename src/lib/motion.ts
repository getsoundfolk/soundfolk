// SPDX-License-Identifier: MPL-2.0
// Durations for the animations Svelte runs in JavaScript (`animate:`, transitions), which the CSS
// `prefers-reduced-motion` rules cannot reach (docs/07 §7): none when the desktop asks for less
// motion. Read at each animation, so a change of the setting applies at once.

/** The duration of `--duration-fast`, in ms. */
export const FAST_MS = 150;

/** `ms`, or 0 when the desktop asks for reduced motion. */
export function motionMs(ms: number): number {
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : ms;
}
