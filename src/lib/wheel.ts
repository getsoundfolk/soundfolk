// SPDX-License-Identifier: MPL-2.0
// The wheel is the fine control of the volume: 1 % per mouse notch, where a click on the track
// jumps. A touchpad sends many small pixel deltas: they add up, one step per notch's worth.

/** Pixels a mouse notch is worth. */
const NOTCH_PX = 100;
/** `WheelEvent.DOM_DELTA_LINE` and `DOM_DELTA_PAGE`, without needing a DOM. */
const DELTA_LINE = 1;
const DELTA_PAGE = 2;

/** What `wheelSteps` needs of a wheel event. */
export type Wheel = { deltaX: number; deltaY: number; deltaMode: number };

/** The volume steps a wheel event makes (positive: down), and the pixels carried over to the
 *  next event. `carry` is what the previous call returned, 0 at first. */
export function wheelSteps(carry: number, e: Wheel): { steps: number; carry: number } {
  const raw = e.deltaY || -e.deltaX;
  if (!raw) return { steps: 0, carry };
  const px =
    e.deltaMode === DELTA_LINE ? raw * 33 : e.deltaMode === DELTA_PAGE ? raw * NOTCH_PX * 3 : raw;
  // A direction change drops what was left over from the other way.
  let total = Math.sign(px) !== Math.sign(carry) ? px : carry + px;
  // At least one step per event: a mouse notch must never do nothing.
  const steps = Math.trunc(total / NOTCH_PX) || (Math.abs(px) >= NOTCH_PX / 2 ? Math.sign(px) : 0);
  if (steps) total = Math.abs(total) >= NOTCH_PX ? total - steps * NOTCH_PX : 0;
  return { steps, carry: total };
}
