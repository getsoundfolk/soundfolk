<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- A curated list's screen (docs/15-curated-lists.md §6.4): its stations in the curator's order,
     ★, Surprise me and an M3U export. Opened from its Discover row, a favorite, the station sheet
     or "In N lists". -->
<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorMessage, type CuratedList, type Station } from '../api';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { pickRandom, playableOf, shortMeta, warningOf } from '../stations';
  import { ui } from '../ui.svelte';
  import StationRow from './StationRow.svelte';
  import Icon from './Icon.svelte';
  import styles from './ListScreen.module.css';

  let { list }: { list: CuratedList } = $props();

  let stations = $state<Station[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let exporting = $state(false);

  // Names and streams as Radio-Browser has them today, asked once per list (lists store).
  $effect(() => {
    const id = list.id;
    void lists.version;
    stations = [];
    error = null;
    notice = null;
    let current = true;
    loading = true;
    lists
      .stations(id)
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

  /** A station of the list at random, a healthy one when there is one, never the one playing. */
  function surprise() {
    const healthy = stations.filter((s) => s.health.status === 'ok');
    const pick = pickRandom(
      healthy.length ? healthy : stations,
      (s) => s.id === player.current?.id,
    );
    if (pick) player.play(playableOf(pick));
  }

  async function exportM3u() {
    if (exporting) return;
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

  function toggleFavorite() {
    lists.toggleFavorite(list.id).catch((e) => (error = errorMessage(e)));
  }

  function open(url: string) {
    openUrl(url).catch((e) => (error = errorMessage(e)));
  }

  const favorite = $derived(lists.isFavorite(list.id));
</script>

<div class={styles.screen}>
  <button class={styles.back} type="button" onclick={() => ui.openList(null)}
    ><Icon name="back" /> Discover</button
  >
  <header class={styles.header}>
    <div class={styles.titleLine}>
      <h2 class={styles.title}>{list.title}</h2>
      <button
        class={styles.star}
        type="button"
        aria-pressed={favorite}
        aria-label={favorite
          ? `Remove ${list.title} from favorites`
          : `Add ${list.title} to favorites`}
        title={favorite ? 'Remove from favorites' : 'Add to favorites'}
        data-on={favorite}
        onclick={toggleFavorite}><Icon name="star" size={18} filled={favorite} /></button
      >
    </div>
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
    <button class={styles.button} type="button" disabled={stations.length === 0} onclick={surprise}
      >Surprise me</button
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
          meta={shortMeta(s)}
          warning={warningOf(s)}
          active={player.current?.id === s.id}
          favorite={player.isFavorite(s.id)}
          onplay={() => player.play(playableOf(s))}
          onfavorite={() => player.toggleFavorite(playableOf(s))}
        />
      {/each}
    </ul>
  {/if}
</div>
