<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- A curated list as a Discover row (docs/10-discovery.md §5.2): its title with its curators, one
     line of description, "See all" (the list's screen, which also has Surprise me) and its first
     stations to play in one click. -->
<script lang="ts">
  import type { CuratedList, Station } from '../api';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { playableOf } from '../stations';
  import { stationNames } from '../stationNames.svelte';
  import { ui } from '../ui.svelte';
  import Icon from './Icon.svelte';
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

  const favorite = $derived(lists.isFavorite(list.id));
</script>

<section class={styles.row} aria-labelledby={`row-${list.id}`}>
  <header class={styles.header}>
    <div class={styles.text}>
      <h3 class={styles.heading}>
        <button
          class={styles.title}
          id={`row-${list.id}`}
          type="button"
          onclick={() => ui.openList(list.id)}>{list.title}</button
        >
        <span class={styles.by}>by {list.curators.map((c) => `@${c}`).join(', ')}</span>
      </h3>
      <p class={styles.description}>{list.description}</p>
    </div>
    <button
      class={styles.link}
      type="button"
      aria-label={`See all ${list.items.length} stations of ${list.title}`}
      onclick={() => ui.openList(list.id)}>See all {list.items.length}</button
    >
    <button
      class={styles.star}
      type="button"
      aria-pressed={favorite}
      aria-label={favorite
        ? `Remove ${list.title} from favorites`
        : `Add ${list.title} to favorites`}
      title={favorite ? 'Remove from favorites' : 'Add to favorites'}
      data-on={favorite}
      onclick={() => lists.toggleFavorite(list.id).catch(() => {})}
      ><Icon name="star" filled={favorite} /></button
    >
  </header>

  <ul class={styles.strip}>
    {#each stations.slice(0, SHOWN) as s (s.id)}
      <li>
        <button
          class={styles.tile}
          type="button"
          title={`Play ${stationNames.shown(s.id, s.name)}`}
          data-active={player.current?.id === s.id}
          data-down={s.health.status === 'unreachable'}
          onclick={() => player.play(playableOf(s))}
        >
          <span class={styles.name}>{stationNames.shown(s.id, s.name)}</span>
          <span class={styles.small}
            >{#if player.current?.id === s.id}<span class={styles.onAir} aria-hidden="true"
              ></span>On air{:else}{s.country_code ?? ''}{/if}</span
          >
        </button>
      </li>
    {:else}
      <li class={styles.placeholder}>
        {failed ? 'The stations could not be loaded.' : 'Loading the stations…'}
      </li>
    {/each}
  </ul>
</section>
