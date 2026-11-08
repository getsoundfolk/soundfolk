// SPDX-License-Identifier: MPL-2.0
// Moving one item of an ordered list, for the lists the listener reorders.

/** A copy of `list` with the item at `from` moved to `to` (the others shift to make room). */
export function moved<T>(list: readonly T[], from: number, to: number): T[] {
  const next = [...list];
  next.splice(to, 0, ...next.splice(from, 1));
  return next;
}
