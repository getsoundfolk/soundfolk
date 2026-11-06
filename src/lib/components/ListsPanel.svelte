<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Curated lists (docs/14-curated-lists.md): every list by title, and one list's screen with its
     stations in the curator's order, Surprise me and an M3U export. Until Discover exists (09),
     they live in their own tab. -->
<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorMessage, type Playable, type Station } from '../api';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { ui } from '../ui.svelte';
  import StationRow from './StationRow.svelte';
  import styles from './ListsPanel.module.css';

  const list = $derived(ui.listId ? lists.get(ui.listId) : null);

  let stations = $state<Station[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let exporting = $state(false);

  // The stations come from the catalog each time a list is opened: names and streams as
  // Radio-Browser has them today.
  $effect(() => {
    const id = list?.id;
    void lists.version;
    stations = [];
    error = null;
    notice = null;
    if (!id) return;
    let current = true;
    loading = true;
    api
      .getCuratedListStations(id)
      .then((found) => {
        if (current) stations = found;
      })
      .catch((e) => {
        if (current) error = errorMessage(e);
      })
      .finally(() => {
        if (current) loading = false;
      });
    return () => {
      current = false;
    };
  });

  const playable = (s: Station): Playable => ({
    id: s.id,
    name: s.name,
    url: s.url,
    genre: s.tags[0] ?? null,
    homepage: s.homepage,
  });

  const meta = (s: Station) =>
    [s.country_code, s.tags.slice(0, 2).join(', ')].filter(Boolean).join(' · ');

  const warning = (s: Station) =>
    s.health.status === 'unreachable' ? 'Unreachable at last check' : null;

  /** A station of the list at random, a healthy one when there is one, never the one playing. */
  function surprise() {
    const healthy = stations.filter((s) => s.health.status === 'ok');
    const pool = (healthy.length ? healthy : stations).filter((s) => s.id !== player.current?.id);
    const pick = pool[Math.floor(Math.random() * pool.length)];
    if (pick) player.play(playable(pick));
  }

  async function exportM3u() {
    if (!list || exporting) return;
    exporting = true;
    error = null;
    try {
      const file = await api.exportCuratedList(list.id);
      notice = `${file.count} ${file.count === 1 ? 'station' : 'stations'} saved to Downloads: ${file.file_name}`;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      exporting = false;
    }
  }

  function open(url: string) {
    openUrl(url).catch((e) => (error = errorMessage(e)));
  }
</script>

{#if list}
  <div class={styles.screen}>
    <button class={styles.back} type="button" onclick={() => ui.openList(null)}>‹ All lists</button>
    <header class={styles.header}>
      <h2 class={styles.title}>{list.title}</h2>
      <p class={styles.description}>{list.description}</p>
      <p class={styles.meta}>
        by
        {#each list.curators as login, i (login)}{#if i > 0},
          {/if}<button
            class={styles.curator}
            type="button"
            title={`github.com/${login}`}
            onclick={() => open(`https://github.com/${login}`)}>@{login}</button
          >{/each}
        · {list.items.length}
        {list.items.length === 1 ? 'station' : 'stations'}
      </p>
    </header>
    <div class={styles.actions}>
      <button
        class={styles.button}
        type="button"
        disabled={stations.length === 0}
        onclick={surprise}>Surprise me</button
      >
      <button
        class={styles.button}
        type="button"
        disabled={stations.length === 0 || exporting}
        onclick={exportM3u}>Export M3U</button
      >
    </div>
    {#if notice}<p class={styles.notice} role="status">{notice}</p>{/if}
    {#if error}<p class={styles.error} role="alert">{error}</p>{/if}

    {#if loading && stations.length === 0}
      <p class={styles.empty}>Loading the stations…</p>
    {:else}
      <ul class={styles.stations}>
        {#each stations as s (s.id)}
          <StationRow
            name={s.name}
            meta={meta(s)}
            warning={warning(s)}
            active={player.current?.id === s.id}
            favorite={player.isFavorite(s.id)}
            onplay={() => player.play(playable(s))}
            onfavorite={() => player.toggleFavorite(playable(s))}
          />
        {/each}
      </ul>
    {/if}
  </div>
{:else if lists.all.length === 0}
  <p class={styles.empty}>
    No lists yet. They are downloaded once a day from soundfolk-stations; check your connection.
  </p>
{:else}
  <ul class={styles.lists}>
    {#each lists.all as l (l.id)}
      <li>
        <button class={styles.card} type="button" onclick={() => ui.openList(l.id)}>
          <span class={styles.cardTitle}>{l.title}</span>
          <span class={styles.description}>{l.description}</span>
          <span class={styles.meta}
            >by {l.curators.map((c) => `@${c}`).join(', ')} · {l.items.length}
            {l.items.length === 1 ? 'station' : 'stations'}</span
          >
        </button>
      </li>
    {/each}
  </ul>
{/if}
