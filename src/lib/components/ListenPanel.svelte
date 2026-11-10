<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { api, errorMessage, type PersonalStation, type Playable } from '../api';
  import { errors } from '../errors.svelte';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { Reorder } from '../reorder.svelte';
  import { hostOf, pickRandom, playableOfSaved, stationCount } from '../stations';
  import { stationNames } from '../stationNames.svelte';
  import { ui } from '../ui.svelte';
  import AddStationForm from './AddStationForm.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import StationRow from './StationRow.svelte';
  import Icon from './Icon.svelte';
  import styles from './StationBrowser.module.css';

  // What is mine (docs/06-ui-principles.md §1): favorites, then the stations added by address.
  let {
    personal,
    onpersonalchanged,
    ondiscover,
  }: {
    personal: PersonalStation[];
    /** A personal station was added or removed: the list is read again. */
    onpersonalchanged: () => void;
    ondiscover: () => void;
  } = $props();

  let adding = $state(false);

  async function removePersonal(station: PersonalStation) {
    errors.clear('stations');
    try {
      await api.removePersonalStation(station.station_id);
      if (player.isFavorite(station.station_id))
        await player.toggleFavorite(playableOfSaved(station));
      onpersonalchanged();
    } catch (e) {
      errors.report('stations', e);
    }
  }

  // ── Favorites and My stations as lists: Surprise me, Export M3U (14 §6.4) ─────────────
  let listNotice = $state<string | null>(null);
  let listError = $state<string | null>(null);

  function surpriseFrom(pool: Playable[]) {
    const pick = pickRandom(pool, (p) => p.id === player.current?.id);
    if (pick) player.play(pick);
  }

  async function exportMine(list: 'favorites' | 'my_stations') {
    listNotice = null;
    listError = null;
    try {
      const file = await api.exportMyList(list);
      listNotice = `${stationCount(file.count)} saved to Downloads: ${file.file_name}`;
    } catch (e) {
      listError = errorMessage(e);
    }
  }

  // Favorites in the listener's order: drag a row's handle to move it.
  const favoriteOrder = new Reorder(
    () => player.favorites.map((f) => f.station_id),
    (id) => {
      const f = player.favorites.find((f) => f.station_id === id);
      return f ? stationNames.shown(f.station_id, f.name) : id;
    },
    (ids) => player.reorderFavorites(ids),
  );
  const shownFavorites = $derived(favoriteOrder.shown(player.favorites, (f) => f.station_id));

  const isActive = (id: string) => player.current?.id === id;
  const favoriteCount = $derived(player.favorites.length + lists.favorites.length);
</script>

{#if listNotice}<p class={styles.notice} role="status">{listNotice}</p>{/if}
{#if listError}<p class={styles.error} role="alert">{listError}</p>{/if}

<section class={styles.section} aria-labelledby="favorites-title">
  <header class={styles.sectionHeader}>
    <h2 class={styles.sectionTitle} id="favorites-title">Favorites</h2>
    {#if player.favorites.length}
      <button
        class={styles.link}
        type="button"
        onclick={() => surpriseFrom(player.favorites.map(playableOfSaved))}>Surprise me</button
      >
      <button class={styles.link} type="button" onclick={() => exportMine('favorites')}
        >Export M3U</button
      >
    {/if}
  </header>
  {#if favoriteCount === 0}
    <p class={styles.hint}>
      Star a station or a list to keep it here.
      <button class={styles.link} type="button" onclick={ondiscover}>Discover stations</button>
    </p>
  {/if}
  {#if lists.favorites.length}
    <ul class={styles.cards}>
      {#each lists.favorites as l (l.id)}
        <li class={styles.card}>
          <button class={styles.cardOpen} type="button" onclick={() => ui.openList(l.id)}>
            <span class={styles.cardTitle}>{l.title}</span>
            <span class={styles.cardMeta}
              >List by {l.curators.map((c) => `@${c}`).join(', ')} · {stationCount(
                l.items.length,
              )}</span
            >
          </button>
          <button
            class={styles.star}
            type="button"
            aria-pressed="true"
            aria-label={`Remove ${l.title} from favorites`}
            title="Remove from favorites"
            onclick={() => lists.toggleFavorite(l.id).catch((e) => errors.report('player', e))}
            ><Icon name="star" filled /></button
          >
        </li>
      {/each}
    </ul>
  {/if}
  {#if player.favorites.length}
    <ul class={styles.list} {@attach favoriteOrder.attach}>
      {#each shownFavorites as f (f.station_id)}
        <StationRow
          name={stationNames.shown(f.station_id, f.name)}
          meta={f.station_id.startsWith('user:') ? (f.genre ?? '') : ''}
          personal={f.station_id.startsWith('user:')}
          active={isActive(f.station_id)}
          favorite
          onplay={() => player.play(playableOfSaved(f))}
          onfavorite={() => player.toggleFavorite(playableOfSaved(f))}
          handle={player.favorites.length > 1 ? favoriteOrder.handle(f.station_id) : undefined}
          dragged={favoriteOrder.draggedId === f.station_id}
        />
      {/each}
    </ul>
    <p class={styles.live} aria-live="polite">{favoriteOrder.announcement}</p>
  {/if}
</section>

<section class={styles.section} aria-labelledby="mine-title">
  <header class={styles.sectionHeader}>
    <h2 class={styles.sectionTitle} id="mine-title">My stations</h2>
    {#if personal.length}
      <button
        class={styles.link}
        type="button"
        onclick={() => surpriseFrom(personal.map(playableOfSaved))}>Surprise me</button
      >
      <button class={styles.link} type="button" onclick={() => exportMine('my_stations')}
        >Export M3U</button
      >
    {/if}
    <button
      class={styles.link}
      type="button"
      aria-expanded={adding}
      onclick={() => (adding = !adding)}
      >{#if !adding}<Icon name="plus" size={14} />{/if}{adding ? 'Cancel' : 'Add a station'}</button
    >
  </header>
  <ErrorBanner area="stations" />
  {#if adding}
    <AddStationForm
      onadded={() => {
        adding = false;
        onpersonalchanged();
      }}
    />
  {/if}
  {#if personal.length === 0 && !adding}
    <p class={styles.hint}>Stations you add by their stream address. They stay on this device.</p>
  {:else}
    <ul class={styles.list}>
      {#each personal as p (p.station_id)}
        <StationRow
          name={stationNames.shown(p.station_id, p.name)}
          meta={p.genre ?? hostOf(p.stream_url)}
          personal
          active={isActive(p.station_id)}
          favorite={player.isFavorite(p.station_id)}
          onplay={() => player.play(playableOfSaved(p))}
          onfavorite={() => player.toggleFavorite(playableOfSaved(p))}
          onremove={() => removePersonal(p)}
        />
      {/each}
    </ul>
  {/if}
</section>
