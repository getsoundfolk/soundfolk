// SPDX-License-Identifier: MPL-2.0
// Keyboard shortcuts of the main window: Space plays or pauses, M mutes, Ctrl+F / Ctrl+K go to
// the search, Up and Down move between the stations of a list. None of them takes a key from a
// field being typed in, nor from a control that already answers it.
import { player } from './player.svelte';
import { ui } from './ui.svelte';

/** Where a letter is typed, not a command. */
const TEXT =
  'input:not([type="range"]):not([type="checkbox"]):not([type="radio"]), textarea, select, [contenteditable]';
/** Controls that Space activates themselves (a button, a tab, a slider…). */
const SPACE_TAKEN = `${TEXT}, button, a[href], summary, input, [role="tab"], [role="radio"], [role="slider"], [role="menuitem"], [role="button"], [role="checkbox"], [role="switch"]`;

export function shortcutKeydown(e: KeyboardEvent) {
  if (e.defaultPrevented || e.isComposing) return;
  const target = e.target instanceof Element ? e.target : null;
  const ctrl = e.ctrlKey || e.metaKey;

  if (ctrl && !e.altKey && !e.shiftKey && (e.key === 'f' || e.key === 'k')) {
    e.preventDefault();
    ui.focusSearch();
    return;
  }
  if (ctrl || e.altKey) return;

  if (e.key === ' ' && !target?.closest(SPACE_TAKEN)) {
    e.preventDefault();
    player.toggle();
  } else if ((e.key === 'm' || e.key === 'M') && !target?.closest(TEXT)) {
    e.preventDefault();
    player.toggleMute();
  } else if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && target) {
    if (moveInList(target, e.key === 'ArrowDown')) e.preventDefault();
  }
}

/** From a control in a station row, to the same control of the next (or previous) row: Tab goes
 *  through the buttons of a row, the arrows through the rows. Returns whether the focus moved. */
function moveInList(target: Element, down: boolean): boolean {
  const row = target.closest<HTMLElement>('[data-station-row]');
  if (!row || target.closest(TEXT)) return false;
  const rows = [
    ...(row.parentElement?.querySelectorAll<HTMLElement>(':scope > [data-station-row]') ?? []),
  ];
  const next = rows[rows.indexOf(row) + (down ? 1 : -1)];
  if (!next) return false;
  const column = [...row.querySelectorAll('button')].indexOf(target as HTMLButtonElement);
  const buttons = [...next.querySelectorAll<HTMLButtonElement>('button')];
  const button = buttons[Math.min(Math.max(column, 0), buttons.length - 1)];
  if (!button) return false;
  button.focus();
  button.scrollIntoView({ block: 'nearest' });
  return true;
}
