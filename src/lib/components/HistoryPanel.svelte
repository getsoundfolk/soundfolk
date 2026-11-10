<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { rovingKeydown } from '../a11y';
  import { api, errorMessage, type HistoryEntry, type LikedTrack } from '../api';
  import { dayLabel, utc } from '../days';
  import { errors } from '../errors.svelte';
  import { history } from '../history.svelte';
  import { player } from '../player.svelte';
  import { displayTitle } from '../trackLinks';
  import { trackServices } from '../trackServices.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import Icon from './Icon.svelte';
  import styles from './HistoryPanel.module.css';
  import StatsPanel from './StatsPanel.svelte';
  import TrackMenu from './TrackMenu.svelte';

  const LIMIT = 200;

  trackServices.load('history');

  type View = 'all' | 'liked' | 'stats';
  const VIEWS: { id: View; label: string }[] = [
    { id: 'all', label: 'All' },
    { id: 'liked', label: 'Liked' },
    { id: 'stats', label: 'Stats' },
  ];

  /** A history entry or a liked track, as a row shows it. */
  interface Row {
    id: string;
    historyId?: number;
    track_key: string;
    artist: string | null;
    title: string;
    station_id: string;
    station_name: string | null;
    at: Date;
    liked: boolean;
    search_query: string;
  }

  let view = $state<View>('all');
  let text = $state('');
  let rows = $state<Row[]>([]);
  let loaded = $state(false);
  /** Why the list could not be read: shown in its place, gone once a reload works. */
  let loadError = $state<string | null>(null);
  /** Row whose action menu is open. */
  let menuId = $state<string | null>(null);
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  // Id of the latest request: an older response is ignored.
  let requestId = 0;

  const fromEntry = (e: HistoryEntry): Row => ({
    id: `h${e.id}`,
    historyId: e.id,
    track_key: e.track_key,
    artist: e.artist,
    title: e.title,
    station_id: e.station_id,
    station_name: e.station_name,
    at: utc(e.played_at),
    liked: e.liked,
    search_query: e.search_query,
  });

  const fromLike = (l: LikedTrack): Row => ({
    id: `l${l.track_key}`,
    track_key: l.track_key,
    artist: l.artist,
    title: l.title,
    station_id: l.station_id,
    station_name: l.station_name,
    at: utc(l.liked_at),
    liked: true,
    search_query: l.search_query,
  });

  async function load(current: 'all' | 'liked', query: string) {
    const mine = ++requestId;
    try {
      const q = query.trim() || null;
      const next =
        current === 'all'
          ? (await api.getListeningHistory(LIMIT, q)).map(fromEntry)
          : (await api.listLikes(LIMIT, q)).map(fromLike);
      if (mine !== requestId) return;
      rows = next;
      loadError = null;
    } catch (e) {
      if (mine === requestId) loadError = errorMessage(e);
    } finally {
      if (mine === requestId) loaded = true;
    }
  }

  // Reload on a new title (the backend logs it before announcing it), a like change, or a new
  // filter; typing waits for a pause.
  $effect(() => {
    void player.nowPlaying.title;
    void history.version;
    const [current, query] = [view, text];
    if (current === 'stats') return;
    const timer = setTimeout(() => load(current, query), query.trim() ? 250 : 0);
    return () => clearTimeout(timer);
  });

  // Today's date, moved on at midnight so the day labels follow while the app stays open.
  // The timer is late after a sleep; showing the window again checks the date too.
  let today = $state(new Date());
  $effect(() => {
    const roll = () => {
      if (new Date().toDateString() !== today.toDateString()) today = new Date();
    };
    const next = new Date(today.getFullYear(), today.getMonth(), today.getDate() + 1);
    const timer = setTimeout(roll, next.getTime() - Date.now() + 1000);
    const onVisible = () => document.visibilityState === 'visible' && roll();
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisible);
    };
  });

  const groups = $derived.by(() => {
    const result: { label: string; rows: Row[] }[] = [];
    for (const row of rows) {
      const label = dayLabel(row.at, today);
      if (result.at(-1)?.label === label) result.at(-1)!.rows.push(row);
      else result.push({ label, rows: [row] });
    }
    return result;
  });

  const time = (r: Row) =>
    r.at.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  const meta = (r: Row) =>
    // A station whose name was never known shows nothing rather than "Unknown" (06 §7).
    [r.artist, r.station_name, time(r)].filter(Boolean).join(' · ');

  function notify(message: string, ms = 1800) {
    toast = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), ms);
  }

  /** Copies the title of a row (double-click). */
  async function copyTitle(row: Row) {
    errors.clear('history');
    try {
      await writeText(displayTitle(row));
      notify('Title copied');
    } catch (e) {
      errors.report('history', e);
    }
  }

  let exporting = $state(false);

  /** Saves the whole list of this view (with its filter, not only the rows shown) to a file. */
  async function exportAs(format: 'csv' | 'json') {
    if (exporting || view === 'stats') return;
    exporting = true;
    errors.clear('history');
    try {
      const list = view === 'all' ? 'history' : 'likes';
      const file = await api.exportHistory(list, format, text.trim() || null);
      notify(
        `${file.count} ${file.count === 1 ? 'track' : 'tracks'} saved to Downloads: ${file.file_name}`,
        4000,
      );
    } catch (e) {
      errors.report('history', e);
    } finally {
      exporting = false;
    }
  }
</script>

<div class={styles.bar}>
  <div
    class={styles.views}
    role="radiogroup"
    tabindex="-1"
    aria-label="Show"
    onkeydown={rovingKeydown}
  >
    {#each VIEWS as v (v.id)}
      <button
        class={styles.view}
        type="button"
        role="radio"
        aria-checked={view === v.id}
        tabindex={view === v.id ? 0 : -1}
        onclick={() => ((view = v.id), (menuId = null))}>{v.label}</button
      >
    {/each}
  </div>
  {#if view !== 'stats'}
    <input
      class={styles.filter}
      type="search"
      placeholder="Filter"
      aria-label="Filter the history"
      bind:value={text}
      spellcheck="false"
      autocomplete="off"
    />
  {/if}
</div>

<ErrorBanner area="history" />

{#if view === 'stats'}
  <StatsPanel />
{:else if loadError}
  <p class={styles.loadError} role="alert">{loadError}</p>
{:else if loaded && rows.length === 0}
  <p class={styles.empty}>
    {#if text.trim()}
      Nothing matches “{text.trim()}”.
    {:else if view === 'liked'}
      No liked tracks yet. Like a title to keep it here.
    {:else}
      No tracks yet. The titles announced by the stations you listen to will appear here.
    {/if}
  </p>
{:else}
  {#each groups as group (group.label)}
    <section class={styles.group} aria-label={group.label}>
      <h3 class={styles.day}>{group.label}</h3>
      <ul class={styles.list}>
        {#each group.rows as row (row.id)}
          <li class={styles.row}>
            <button
              class={styles.like}
              type="button"
              aria-pressed={row.liked}
              aria-label={row.liked ? `Unlike ${row.title}` : `Like ${row.title}`}
              data-on={row.liked}
              onclick={() => history.toggleLike(row, 'history')}
              ><Icon name="heart" filled={row.liked} /></button
            >

            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class={styles.text} title="Double-click to copy" ondblclick={() => copyTitle(row)}>
              <p class={styles.title} title={row.title}>{row.title}</p>
              <p class={styles.meta} title={meta(row)}>{meta(row)}</p>
            </div>

            <TrackMenu
              track={row}
              open={menuId === row.id}
              ontoggle={() => (menuId = menuId === row.id ? null : row.id)}
              onclose={() => (menuId = null)}
              onnotify={notify}
            />
          </li>
        {/each}
      </ul>
    </section>
  {/each}

  {#if rows.length > 0}
    <div class={styles.footer}>
      {#each ['csv', 'json'] as const as format (format)}
        <button
          class={styles.export}
          type="button"
          disabled={exporting}
          title={text.trim()
            ? `Save the tracks matching “${text.trim()}” to Downloads`
            : 'Save to the Downloads folder'}
          onclick={() => exportAs(format)}>Export {format.toUpperCase()}</button
        >
      {/each}
    </div>
  {/if}
{/if}

<p class={styles.toast} role="status" aria-live="polite" data-visible={toast !== null}>
  {toast ?? ''}
</p>
