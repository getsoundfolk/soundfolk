<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import type { PersonalStation } from '../api';
  import { catalogSearch as search } from '../catalogSearch.svelte';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { playableOf, playableOfSaved, shortMeta, stationCount, warningOf } from '../stations';
  import { stationNames } from '../stationNames.svelte';
  import { share } from '../share.svelte';
  import { ui } from '../ui.svelte';
  import ListRow from './ListRow.svelte';
  import StationRow from './StationRow.svelte';
  import Icon from './Icon.svelte';
  import styles from './StationBrowser.module.css';

  // What others offer (docs/06-ui-principles.md §1): the lists, and a search of the catalog.
  let {
    personal,
    searchField = $bindable(),
  }: {
    /** Searched too, so a station of mine is found where I look for stations. */
    personal: PersonalStation[];
    searchField?: HTMLInputElement;
  } = $props();

  // The catalog is only queried once text is typed, and not again for the same text; without
  // text, Discover shows its rows.
  $effect(() => search.follow());

  // Typing: the matching lists first, as one row, then the stations (14 §7 D).
  const matchingLists = $derived(lists.matching(search.text));

  const matchingPersonal = $derived.by(() => {
    const query = search.text.trim().toLowerCase();
    if (query === '') return [];
    return personal.filter((p) =>
      [p.name, stationNames.shown(p.station_id, p.name)].some((n) =>
        n.toLowerCase().includes(query),
      ),
    );
  });

  /** A share link pasted into the search opens what it carries instead of searching (16 §3.1). */
  async function pasteLink(e: ClipboardEvent) {
    const pasted = e.clipboardData?.getData('text/plain') ?? '';
    if (!share.mayBeLink(pasted)) return;
    e.preventDefault();
    if (await share.openText(pasted)) return;
    // Another web address: searched like any text.
    search.text = pasted.trim();
  }

  const isActive = (id: string) => player.current?.id === id;
</script>

<div class={styles.searchRow}>
  <input
    class={styles.search}
    type="search"
    bind:this={searchField}
    placeholder="Search stations and lists"
    aria-label="Search stations and lists"
    bind:value={search.text}
    onpaste={pasteLink}
    spellcheck="false"
    autocomplete="off"
  />
  <button
    class={styles.surprise}
    type="button"
    disabled={player.surprising}
    title={player.surprising ? 'Finding a station…' : 'Play a random station'}
    onclick={() => player.surprise()}><Icon name="shuffle" size={16} />Surprise me</button
  >
</div>

{#if search.text.trim() === ''}
  {#each lists.all as l (l.id)}
    <ListRow list={l} />
  {:else}
    <p class={styles.empty}>
      No lists yet. They are downloaded once a day from soundfolk-stations; type a name to search
      the stations.
    </p>
  {/each}
{:else}
  {#if matchingLists.length}
    <h2 class={styles.resultsTitle}>Lists</h2>
    <ul class={styles.cardStrip} aria-label="Matching lists">
      {#each matchingLists as l (l.id)}
        <li class={styles.card}>
          <button class={styles.cardOpen} type="button" onclick={() => ui.openList(l.id)}>
            <span class={styles.cardTitle}>{l.title}</span>
            <span class={styles.cardMeta}>List · {stationCount(l.items.length)}</span>
          </button>
        </li>
      {/each}
    </ul>
    <h2 class={styles.resultsTitle}>Stations</h2>
  {/if}

  <ul class={styles.list}>
    {#each matchingPersonal as p (p.station_id)}
      <StationRow
        name={stationNames.shown(p.station_id, p.name)}
        personal
        active={isActive(p.station_id)}
        favorite={player.isFavorite(p.station_id)}
        onplay={() => player.play(playableOfSaved(p))}
        onfavorite={() => player.toggleFavorite(playableOfSaved(p))}
      />
    {/each}
    {#each search.results as s (s.id)}
      <StationRow
        name={stationNames.shown(s.id, s.name)}
        meta={shortMeta(s)}
        warning={warningOf(s)}
        active={isActive(s.id)}
        favorite={player.isFavorite(s.id)}
        onplay={() => player.play(playableOf(s))}
        onfavorite={() => player.toggleFavorite(playableOf(s))}
        oninfo={() => ui.showStation(s)}
      />
    {/each}
  </ul>

  {#if search.error}
    <p class={styles.error} role="alert">{search.error}</p>
  {:else if search.loading && search.results.length === 0}
    <p class={styles.empty}>Searching…</p>
  {:else if !search.loading && search.results.length === 0 && matchingPersonal.length === 0}
    <p class={styles.empty}>No stations found.</p>
  {/if}

  {#if search.hasMore && !search.error}
    <button
      class={styles.more}
      type="button"
      disabled={search.loading}
      aria-busy={search.loading}
      title={search.loading ? 'Loading…' : undefined}
      onclick={() => search.search(false)}>Show more</button
    >
  {/if}
{/if}
