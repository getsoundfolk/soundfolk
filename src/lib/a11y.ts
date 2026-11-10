// SPDX-License-Identifier: MPL-2.0
// Keyboard patterns from the WAI-ARIA Authoring Practices.

/** Index of the item a navigation key moves to, or null for any other key. Wraps around. */
export function target(
  key: string,
  current: number,
  count: number,
  vertical: boolean,
): number | null {
  const [prev, next] = vertical ? ['ArrowUp', 'ArrowDown'] : ['ArrowLeft', 'ArrowRight'];
  switch (key) {
    case prev:
      return (current - 1 + count) % count;
    case next:
      return (current + 1) % count;
    case 'Home':
      return 0;
    case 'End':
      return count - 1;
    default:
      return null;
  }
}

/**
 * Arrow keys on a tablist or a radiogroup (roving tabindex): move the focus to the previous or
 * next tab or radio and select it, as a click would. Only the selected one is in the Tab order
 * (`tabindex={selected ? 0 : -1}`), so Tab leaves the group.
 */
export function rovingKeydown(e: KeyboardEvent) {
  const group = e.currentTarget as HTMLElement;
  const items = [...group.querySelectorAll<HTMLElement>('[role="tab"], [role="radio"]')];
  const current = items.indexOf(document.activeElement as HTMLElement);
  if (current < 0) return;
  // A radiogroup answers both axes; a tablist only its own.
  const horizontal = target(e.key, current, items.length, false);
  const vertical =
    group.getAttribute('role') === 'radiogroup' ? target(e.key, current, items.length, true) : null;
  const next = horizontal ?? vertical;
  if (next === null) return;
  e.preventDefault();
  items[next].focus();
  items[next].click();
}

/** Up and Down arrows, Home and End move the focus between the items of a menu. */
export function menuKeydown(e: KeyboardEvent) {
  const items = [
    ...(e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('[role="menuitem"]'),
  ];
  const current = items.indexOf(document.activeElement as HTMLElement);
  const next = target(e.key, Math.max(current, 0), items.length, true);
  if (next === null || items.length === 0) return;
  e.preventDefault();
  items[current < 0 && e.key === 'ArrowDown' ? 0 : next].focus();
}

/** Focuses the first item of a menu as it opens: `{@attach focusFirstItem}`. */
export function focusFirstItem(menu: HTMLElement) {
  menu.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
}
