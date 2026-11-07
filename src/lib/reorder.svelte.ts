// SPDX-License-Identifier: MPL-2.0
// Reordering a list of rows: drag a row's handle (or focus it and use ↑ / ↓). The rows move while
// dragging; the new order is handed over on release.

import { tick } from 'svelte';

export class Reorder {
  /** The order (ids) while a row is being dragged. */
  draft = $state<string[] | null>(null);
  draggedId = $state<string | null>(null);
  /** For screen readers: where the moved row now stands. */
  announcement = $state('');
  /** The element whose children are the rows, in the order shown (see `attach`). */
  private list: HTMLElement | null = null;

  constructor(
    private ids: () => string[],
    private name: (id: string) => string,
    private onchange: (ids: string[]) => void,
  ) {}

  /** Attachment for the element whose children are the rows: `{@attach reorder.attach}`. */
  attach = (node: HTMLElement) => {
    this.list = node;
    return () => {
      if (this.list === node) this.list = null;
    };
  };

  /** `items` in the order to show: the dragged order while dragging. */
  shown<T>(items: T[], id: (item: T) => string): T[] {
    if (!this.draft) return items;
    const byId = new Map(items.map((item) => [id(item), item]));
    return this.draft.flatMap((i) => byId.get(i) ?? []);
  }

  /** Attributes and handlers of a row's handle. */
  handle(id: string) {
    return {
      'data-reorder': id,
      onpointerdown: (e: PointerEvent) => this.start(e, id),
      onpointermove: (e: PointerEvent) => this.move(e),
      onpointerup: () => this.end(),
      onpointercancel: () => this.end(),
      onkeydown: (e: KeyboardEvent) => this.key(e, id),
    };
  }

  private start(e: PointerEvent, id: string) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    this.draggedId = id;
    this.draft = this.ids();
  }

  private move(e: PointerEvent) {
    const { draft, draggedId, list } = this;
    if (!draft || !draggedId || !list) return;
    // The row under the pointer takes the dragged one's place.
    const to = [...list.children].findIndex((row) => {
      const rect = row.getBoundingClientRect();
      return e.clientY >= rect.top && e.clientY < rect.bottom;
    });
    const from = draft.indexOf(draggedId);
    if (to !== -1 && to < draft.length && to !== from) this.draft = moved(draft, from, to);
  }

  private end() {
    const { draft, draggedId } = this;
    if (!draft || !draggedId) return;
    const ids = this.ids();
    if (draft.some((id, i) => id !== ids[i])) {
      this.onchange(draft);
      this.announce(draft, draggedId);
    }
    this.draft = null;
    this.draggedId = null;
  }

  private async key(e: KeyboardEvent, id: string) {
    const step = e.key === 'ArrowUp' ? -1 : e.key === 'ArrowDown' ? 1 : 0;
    if (!step) return;
    e.preventDefault();
    const ids = this.ids();
    const from = ids.indexOf(id);
    const to = from + step;
    if (from === -1 || to < 0 || to >= ids.length) return;
    const next = moved(ids, from, to);
    this.onchange(next);
    this.announce(next, id);
    // Moving the row in the page may take the focus away from its handle.
    await tick();
    this.list?.querySelector<HTMLElement>(`[data-reorder="${CSS.escape(id)}"]`)?.focus();
  }

  private announce(ids: string[], id: string) {
    this.announcement = `${this.name(id)}, position ${ids.indexOf(id) + 1} of ${ids.length}`;
  }
}

function moved<T>(list: T[], from: number, to: number): T[] {
  const next = [...list];
  next.splice(to, 0, ...next.splice(from, 1));
  return next;
}
