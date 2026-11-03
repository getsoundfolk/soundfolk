<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorMessage, type HistoryEntry } from '../api';
  import { player } from '../player.svelte';
  import { shareText, TRACK_SERVICES } from '../trackLinks';
  import styles from './HistoryPanel.module.css';

  const LIMIT = 200;

  let entries = $state<HistoryEntry[]>([]);
  let loaded = $state(false);
  /** Row whose action menu is open, and whether it opens upwards (row low in the window). */
  let menu = $state<{ id: number; up: boolean } | null>(null);
  let toast = $state<string | null>(null);
  let confirmClear = $state(false);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let confirmTimer: ReturnType<typeof setTimeout> | undefined;

  async function load() {
    try {
      entries = await api.getListeningHistory(LIMIT);
    } catch (e) {
      player.error = errorMessage(e);
    } finally {
      loaded = true;
    }
  }

  // The backend logs a title before announcing it: reloading on each new title keeps the list live.
  $effect(() => {
    void player.nowPlaying.title;
    load();
  });

  // SQLite stores UTC without a zone marker.
  const playedAt = (e: HistoryEntry) => new Date(`${e.played_at.replace(' ', 'T')}Z`);

  function dayLabel(date: Date): string {
    const today = new Date();
    const yesterday = new Date();
    yesterday.setDate(today.getDate() - 1);
    if (date.toDateString() === today.toDateString()) return 'Today';
    if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
    return date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
  }

  const groups = $derived.by(() => {
    const result: { label: string; entries: HistoryEntry[] }[] = [];
    for (const entry of entries) {
      const label = dayLabel(playedAt(entry));
      if (result.at(-1)?.label === label) result.at(-1)!.entries.push(entry);
      else result.push({ label, entries: [entry] });
    }
    return result;
  });

  const time = (e: HistoryEntry) => playedAt(e).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });

  function notify(message: string) {
    toast = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), 1800);
  }

  async function copy(text: string, message: string) {
    menu = null;
    try {
      await writeText(text);
      notify(message);
    } catch (e) {
      player.error = errorMessage(e);
    }
  }

  async function open(url: string) {
    menu = null;
    try {
      await openUrl(url);
    } catch (e) {
      player.error = errorMessage(e);
    }
  }

  function toggleMenu(entry: HistoryEntry, button: HTMLElement) {
    if (menu?.id === entry.id) {
      menu = null;
      return;
    }
    menu = { id: entry.id, up: button.getBoundingClientRect().bottom > window.innerHeight * 0.6 };
  }

  // Two clicks to clear: the history cannot be recovered.
  async function clear() {
    if (!confirmClear) {
      confirmClear = true;
      clearTimeout(confirmTimer);
      confirmTimer = setTimeout(() => (confirmClear = false), 3000);
      return;
    }
    confirmClear = false;
    try {
      await api.clearListeningHistory();
      entries = [];
    } catch (e) {
      player.error = errorMessage(e);
    }
  }
</script>

<!-- Close the menu on Escape or on a click outside it. -->
<svelte:window
  onkeydown={(e) => e.key === 'Escape' && (menu = null)}
  onclick={(e) => menu && !(e.target as Element).closest(`.${styles.actions}`) && (menu = null)}
/>

{#if loaded && entries.length === 0}
  <p class={styles.empty}>No tracks yet. The titles announced by the stations you listen to will appear here.</p>
{:else}
  {#each groups as group (group.label)}
    <section class={styles.group} aria-label={group.label}>
      <h3 class={styles.day}>{group.label}</h3>
      <ul class={styles.list}>
        {#each group.entries as entry (entry.id)}
          <li class={styles.row}>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class={styles.text}
              title="Double-click to copy"
              ondblclick={() => copy(entry.track_title, 'Title copied')}
            >
              <p class={styles.title}>{entry.track_title}</p>
              <p class={styles.meta}>{entry.station_name ?? 'Unknown station'} · {time(entry)}</p>
            </div>

            <div class={styles.actions}>
              <button
                class={styles.more}
                type="button"
                aria-label={`Actions for ${entry.track_title}`}
                aria-haspopup="menu"
                aria-expanded={menu?.id === entry.id}
                onclick={(e) => toggleMenu(entry, e.currentTarget)}>⋯</button
              >
              {#if menu?.id === entry.id}
                <div class={styles.menu} data-up={menu.up} role="menu">
                  <p class={styles.heading}>Find this track on</p>
                  {#each TRACK_SERVICES as service (service.name)}
                    <button class={styles.item} type="button" role="menuitem" onclick={() => open(service.url(entry.track_title))}>
                      {service.name}
                    </button>
                  {/each}
                  <hr class={styles.separator} />
                  <button class={styles.item} type="button" role="menuitem" onclick={() => copy(entry.track_title, 'Title copied')}>
                    Copy title
                  </button>
                  <button
                    class={styles.item}
                    type="button"
                    role="menuitem"
                    onclick={() => copy(shareText(entry.track_title, entry.station_name), 'Copied, ready to share')}
                  >
                    Copy to share
                  </button>
                </div>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    </section>
  {/each}

  {#if entries.length > 0}
    <button class={styles.clear} type="button" data-confirm={confirmClear} onclick={clear}>
      {confirmClear ? 'Click again to clear the history' : 'Clear history'}
    </button>
  {/if}
{/if}

<p class={styles.toast} role="status" aria-live="polite" data-visible={toast !== null}>{toast ?? ''}</p>
