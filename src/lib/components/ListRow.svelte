<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- A curated list as a Discover row (docs/09-discovery.md §5.2): its header, its first stations to
     play in one click, Surprise me and "See all", which opens the list's screen. -->
<script lang="ts">
  import type { CuratedList, Station } from '../api';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { pickRandom, playableOf } from '../stations';
  import { ui } from '../ui.svelte';
  import styles from './ListRow.module.css';

  let { list }: { list: CuratedList } = $props();

  /** Stations shown in the row; the list's screen has them all. */
  const SHOWN = 10;

  let stations = $state<Station[]>([]);
  let failed = $state(false);

  $effect(() => {
    const id = list.id;
    void lists.version;
    let current = true;
    failed = false;
    lists
      .stations(id)
      .then((found) => {
        if (current) stations = found;
      })
      .catch(() => {
        if (current) failed = true;
      });
    return () => {
      current = false;
    };
  });

  const healthy = $derived(stations.filter((s) => s.health.status === 'ok'));
  const favorite = $derived(lists.isFavorite(list.id));

  function surprise() {
    const pick = pickRandom(
      healthy.length ? healthy : stations,
      (s) => s.id === player.current?.id,
    );
    if (pick) player.play(playableOf(pick));
  }
</script>

<section class={styles.row} aria-labelledby={`row-${list.id}`}>
  <header class={styles.header}>
    <div class={styles.text}>
      <button
        class={styles.title}
        id={`row-${list.id}`}
        type="button"
        onclick={() => ui.openList(list.id)}>{list.title}</button
      >
      <p class={styles.meta}>
        by {list.curators.map((c) => `@${c}`).join(', ')} · {list.items.length}
        {list.items.length === 1 ? 'station' : 'stations'}
      </p>
      <p class={styles.description}>{list.description}</p>
    </div>
    <button
      class={styles.star}
      type="button"
      aria-pressed={favorite}
      aria-label={favorite
        ? `Remove ${list.title} from favorites`
        : `Add ${list.title} to favorites`}
      title={favorite ? 'Remove from favorites' : 'Add to favorites'}
      data-on={favorite}
      onclick={() => lists.toggleFavorite(list.id).catch(() => {})}>{favorite ? '★' : '☆'}</button
    >
  </header>

  <ul class={styles.strip}>
    {#each stations.slice(0, SHOWN) as s (s.id)}
      <li>
        <button
          class={styles.tile}
          type="button"
          title={`Play ${s.name}`}
          data-active={player.current?.id === s.id}
          data-down={s.health.status === 'unreachable'}
          onclick={() => player.play(playableOf(s))}
        >
          <span class={styles.name}>{s.name}</span>
          <span class={styles.small}>{s.country_code ?? ''}</span>
        </button>
      </li>
    {:else}
      <li class={styles.placeholder}>
        {failed ? 'The stations could not be loaded.' : 'Loading the stations…'}
      </li>
    {/each}
  </ul>

  <div class={styles.actions}>
    <button class={styles.link} type="button" disabled={stations.length === 0} onclick={surprise}
      >Surprise me</button
    >
    <button class={styles.link} type="button" onclick={() => ui.openList(list.id)}>See all</button>
  </div>
</section>
