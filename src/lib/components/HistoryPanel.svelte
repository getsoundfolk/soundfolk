<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorMessage, type HistoryEntry, type LikedTrack } from '../api';
  import { errors } from '../errors.svelte';
  import { history } from '../history.svelte';
  import { player } from '../player.svelte';
  import { displayTitle, shareText } from '../trackLinks';
  import { trackServices } from '../trackServices.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import styles from './HistoryPanel.module.css';
  import StatsPanel from './StatsPanel.svelte';

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
  /**
   * Row whose action menu is open, and where. The menu is fixed to the window so the scrolling
   * panel cannot clip it; it opens on the side of the button with more room.
   */
  let menu = $state<{ id: string; style: string } | null>(null);
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  // Id of the latest request: an older response is ignored.
  let requestId = 0;

  // SQLite stores UTC without a zone marker.
  const utc = (sqlite: string) => new Date(`${sqlite.replace(' ', 'T')}Z`);

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
      const q = query.trim() || undefined;
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

  function dayLabel(date: Date): string {
    const today = new Date();
    const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
    if (date.toDateString() === today.toDateString()) return 'Today';
    if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
    return date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
  }

  const groups = $derived.by(() => {
    const result: { label: string; rows: Row[] }[] = [];
    for (const row of rows) {
      const label = dayLabel(row.at);
      if (result.at(-1)?.label === label) result.at(-1)!.rows.push(row);
      else result.push({ label, rows: [row] });
    }
    return result;
  });

  const time = (r: Row) =>
    r.at.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  const meta = (r: Row) =>
    [r.artist, r.station_name ?? 'Unknown station', time(r)].filter(Boolean).join(' · ');

  function notify(message: string, ms = 1800) {
    toast = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), ms);
  }

  /** Runs a menu action: closes the menu, reports a failure. */
  async function act(action: () => Promise<unknown>, done?: string) {
    menu = null;
    errors.clear('history');
    try {
      await action();
      if (done) notify(done);
    } catch (e) {
      errors.report('history', e);
    }
  }

  const copy = (text: string, done: string) => act(() => writeText(text), done);
  const resolve = (r: Row) => api.resolveStation(r.station_id, r.station_name ?? undefined);

  function toggleMenu(row: Row, button: HTMLElement) {
    if (menu?.id === row.id) {
      menu = null;
      return;
    }
    const rect = button.getBoundingClientRect();
    const right = `right: ${window.innerWidth - rect.right}px;`;
    const below = window.innerHeight - rect.bottom;
    const style =
      below >= rect.top
        ? `${right} top: ${rect.bottom + 4}px; max-height: ${below - 12}px;`
        : `${right} bottom: ${window.innerHeight - rect.top + 4}px; max-height: ${rect.top - 12}px;`;
    menu = { id: row.id, style };
  }

  // A fixed menu would stay put while its row scrolls away: close it on any scroll, except one
  // of the menu itself (it scrolls when the window is too short to show all its items).
  $effect(() => {
    if (!menu) return;
    const close = (e: Event) => {
      if (e.target instanceof Element && e.target.closest(`.${styles.menu}`)) return;
      menu = null;
    };
    document.addEventListener('scroll', close, { capture: true, passive: true });
    return () => document.removeEventListener('scroll', close, { capture: true });
  });

  let exporting = $state(false);

  /** Saves the whole list of this view (with its filter, not only the rows shown) to a file. */
  async function exportAs(format: 'csv' | 'json') {
    if (exporting || view === 'stats') return;
    exporting = true;
    errors.clear('history');
    try {
      const list = view === 'all' ? 'history' : 'likes';
      const file = await api.exportHistory(list, format, text.trim() || undefined);
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

<!-- Close the menu on Escape or on a click outside it. -->
<svelte:window
  onkeydown={(e) => e.key === 'Escape' && (menu = null)}
  onresize={() => (menu = null)}
  onclick={(e) => menu && !(e.target as Element).closest(`.${styles.actions}`) && (menu = null)}
/>

<div class={styles.bar}>
  <div class={styles.views} role="radiogroup" aria-label="Show">
    {#each VIEWS as v (v.id)}
      <button
        class={styles.view}
        type="button"
        role="radio"
        aria-checked={view === v.id}
        onclick={() => ((view = v.id), (menu = null))}>{v.label}</button
      >
    {/each}
  </div>
  {#if view !== 'stats'}
    <input
      class={styles.filter}
      type="search"
      placeholder="Filter by title, artist, station…"
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
      No liked tracks yet. Tap ♡ next to a title to keep it here.
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
              onclick={() => history.toggleLike(row, 'history')}>{row.liked ? '♥' : '♡'}</button
            >

            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class={styles.text}
              title="Double-click to copy"
              ondblclick={() => copy(displayTitle(row), 'Title copied')}
            >
              <p class={styles.title}>{row.title}</p>
              <p class={styles.meta}>{meta(row)}</p>
            </div>

            <div class={styles.actions}>
              <button
                class={styles.more}
                type="button"
                aria-label={`Actions for ${row.title}`}
                aria-haspopup="menu"
                aria-expanded={menu?.id === row.id}
                onclick={(e) => toggleMenu(row, e.currentTarget)}>⋯</button
              >
              {#if menu?.id === row.id}
                <div class={styles.menu} style={menu.style} role="menu">
                  {#if trackServices.enabled.length}
                    <p class={styles.heading}>Find this track on</p>
                    {#each trackServices.enabled as service (service.id)}
                      <button
                        class={styles.item}
                        type="button"
                        role="menuitem"
                        onclick={() => act(() => openUrl(service.url(row.search_query)))}
                      >
                        {service.name}
                      </button>
                    {/each}
                    <hr class={styles.separator} />
                  {/if}
                  <button
                    class={styles.item}
                    type="button"
                    role="menuitem"
                    onclick={() => copy(displayTitle(row), 'Title copied')}
                  >
                    Copy title
                  </button>
                  <button
                    class={styles.item}
                    type="button"
                    role="menuitem"
                    onclick={() =>
                      copy(
                        shareText(displayTitle(row), row.station_name),
                        'Copied, ready to share',
                      )}
                  >
                    Copy to share
                  </button>
                  <hr class={styles.separator} />
                  <button
                    class={styles.item}
                    type="button"
                    role="menuitem"
                    onclick={() => act(async () => player.play(await resolve(row)))}
                  >
                    Play {row.station_name ?? 'the station'}
                  </button>
                  {#if player.isFavorite(row.station_id)}
                    <button
                      class={styles.item}
                      type="button"
                      role="menuitem"
                      onclick={() =>
                        act(
                          () =>
                            player.toggleFavorite({
                              id: row.station_id,
                              name: row.station_name ?? '',
                              url: '',
                            }),
                          'Station removed from favorites',
                        )}
                    >
                      Remove station from favorites
                    </button>
                  {:else}
                    <button
                      class={styles.item}
                      type="button"
                      role="menuitem"
                      onclick={() =>
                        act(
                          async () => player.toggleFavorite(await resolve(row)),
                          'Station added to favorites',
                        )}
                    >
                      Add station to favorites
                    </button>
                  {/if}
                </div>
              {/if}
            </div>
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
