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
  import { flip } from 'svelte/animate';
  import { FAST_MS, motionMs } from '../motion';
  import { Reorder } from '../reorder.svelte';
  import Icon from './Icon.svelte';
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

  const byId = (id: string) => items.find((i) => i.id === id);
  const order = new Reorder(
    () => items.map((i) => i.id),
    (id) => byId(id)?.name ?? id,
    (ids) => onchange(ids.flatMap((id) => byId(id) ?? [])),
  );
  const shown = $derived(order.shown(items, (i) => i.id));

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
        <Icon name="reset" size={14} /> Restore defaults
      </button>
    {/if}
  </div>

  <ol class={styles.list} {@attach order.attach}>
    {#each shown as item, index (item.id)}
      <li
        class={styles.row}
        data-dragged={item.id === order.draggedId}
        animate:flip={{ duration: motionMs(FAST_MS) }}
      >
        <button
          class={styles.handle}
          type="button"
          aria-label={`Move ${item.name}`}
          title="Drag, or use ↑ ↓, to reorder"
          {...order.handle(item.id)}><Icon name="grip" /></button
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
  <p class={styles.live} aria-live="polite">{order.announcement}</p>
</div>
