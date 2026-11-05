<!-- SPDX-License-Identifier: MPL-2.0 -->
<!--
  Sources asked in order: drag the handle (or focus it and use ↑ / ↓) to reorder, a switch each.
  Reusable for any list of sources ("Find this track on", recipe sources…).
-->
<script lang="ts" module>
  export type SourceItem = {
    id: string;
    name: string;
    /** Short label next to the name, e.g. what the source sends. */
    badge?: string;
    detail: string;
    enabled: boolean;
  };
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { flip } from 'svelte/animate';
  import styles from './OrderedSources.module.css';

  let {
    title,
    intro,
    items,
    isDefault,
    onchange,
    onreset,
  }: {
    title: string;
    intro: string;
    items: SourceItem[];
    /** Order and switches are the default ones: the reset link is hidden. */
    isDefault: boolean;
    onchange: (items: SourceItem[]) => void;
    onreset: () => void;
  } = $props();

  /** The order while a row is being dragged, saved on release. */
  let draft = $state<SourceItem[] | null>(null);
  let draggedId = $state<string | null>(null);
  let announcement = $state('');
  const shown = $derived(draft ?? items);
  const rows = new Map<string, HTMLElement>();
  const handles = new Map<string, HTMLElement>();

  function register(map: Map<string, HTMLElement>, id: string) {
    return (node: HTMLElement) => {
      map.set(id, node);
      return () => map.delete(id);
    };
  }

  const moved = (list: SourceItem[], from: number, to: number) => {
    const next = [...list];
    next.splice(to, 0, ...next.splice(from, 1));
    return next;
  };

  function announce(list: SourceItem[], id: string) {
    const index = list.findIndex((i) => i.id === id);
    announcement = `${list[index].name}, position ${index + 1} of ${list.length}`;
  }

  function startDrag(e: PointerEvent, id: string) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    draggedId = id;
    draft = [...items];
  }

  function drag(e: PointerEvent) {
    if (!draft || !draggedId) return;
    const from = draft.findIndex((i) => i.id === draggedId);
    // The row under the pointer takes the dragged one's place.
    const to = draft.findIndex((i) => {
      const rect = rows.get(i.id)?.getBoundingClientRect();
      return rect && e.clientY >= rect.top && e.clientY < rect.bottom;
    });
    if (to !== -1 && to !== from) draft = moved(draft, from, to);
  }

  function endDrag() {
    if (!draft || !draggedId) return;
    const changed = draft.some((item, i) => item.id !== items[i]?.id);
    if (changed) {
      onchange(draft);
      announce(draft, draggedId);
    }
    draft = null;
    draggedId = null;
  }

  async function onkeydown(e: KeyboardEvent, index: number) {
    const to = e.key === 'ArrowUp' ? index - 1 : e.key === 'ArrowDown' ? index + 1 : null;
    if (to === null) return;
    e.preventDefault();
    if (to < 0 || to >= items.length) return;
    const id = items[index].id;
    const next = moved(items, index, to);
    onchange(next);
    announce(next, id);
    // Moving the row in the page may take the focus away from its handle.
    await tick();
    handles.get(id)?.focus();
  }

  const toggle = (id: string, enabled: boolean) =>
    onchange(items.map((i) => (i.id === id ? { ...i, enabled } : i)));
</script>

<div class={styles.box}>
  <div class={styles.head}>
    <span class={styles.text}>
      <span class={styles.title}>{title}</span>
      <span class={styles.detail}>{intro}</span>
    </span>
    {#if !isDefault}
      <button class={styles.reset} type="button" onclick={onreset}>
        <span aria-hidden="true">↺</span> Restore defaults
      </button>
    {/if}
  </div>

  <ol class={styles.list}>
    {#each shown as item, index (item.id)}
      <li class={styles.row} data-dragged={item.id === draggedId} {@attach register(rows, item.id)} animate:flip={{ duration: 150 }}>
        <button
          class={styles.handle}
          type="button"
          aria-label={`Move ${item.name}, position ${index + 1} of ${shown.length}`}
          title="Drag, or use ↑ ↓, to reorder"
          {@attach register(handles, item.id)}
          onpointerdown={(e) => startDrag(e, item.id)}
          onpointermove={drag}
          onpointerup={endDrag}
          onpointercancel={endDrag}
          onkeydown={(e) => onkeydown(e, index)}>⠿</button
        >
        <span class={styles.rank} aria-hidden="true">{index + 1}</span>
        <label class={styles.option}>
          <span class={styles.text}>
            <span class={styles.name}>
              {item.name}
              {#if item.badge}<span class={styles.badge}>{item.badge}</span>{/if}
            </span>
            <span class={styles.detail}>{item.detail}</span>
          </span>
          <input
            class={styles.toggle}
            type="checkbox"
            role="switch"
            checked={item.enabled}
            onchange={(e) => toggle(item.id, e.currentTarget.checked)}
          />
        </label>
      </li>
    {/each}
  </ol>
  <p class={styles.live} aria-live="polite">{announcement}</p>
</div>
