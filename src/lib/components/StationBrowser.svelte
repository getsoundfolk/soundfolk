<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, type Favorite, type PersonalStation, type Playable, type Station } from '../api';
  import { player } from '../player.svelte';
  import AddStationForm from './AddStationForm.svelte';
  import HistoryPanel from './HistoryPanel.svelte';
  import StationRow from './StationRow.svelte';
  import styles from './StationBrowser.module.css';

  type Tab = 'history' | 'search' | 'favorites' | 'personal';
  const TABS: { id: Tab; label: string }[] = [
    { id: 'history', label: 'History' },
    { id: 'search', label: 'Search' },
    { id: 'favorites', label: 'Favorites' },
    { id: 'personal', label: 'My stations' },
  ];
  const PAGE = 30;

  let tab = $state<Tab>('history');

  // ── Search (online catalog) ──────────────────────────────────────────
  let text = $state('');
  let results = $state<Station[]>([]);
  let loading = $state(false);
  let searchError = $state<string | null>(null);
  // Offset of the next page, or null when the catalog has no more results.
  let nextOffset = $state<number | null>(null);
  const hasMore = $derived(nextOffset !== null);
  // Id of the latest request: an older response is ignored.
  let requestId = 0;

  async function search(reset: boolean) {
    const mine = ++requestId;
    loading = true;
    searchError = null;
    try {
      const page = await api.searchStations({
        text: text.trim() || undefined,
        limit: PAGE,
        // The offset counts the catalog's own entries, not the stations shown (some are filtered out).
        offset: reset ? 0 : (nextOffset ?? 0),
      });
      if (mine !== requestId) return;
      results = reset ? page.stations : [...results, ...page.stations];
      nextOffset = page.next_offset;
    } catch (e) {
      if (mine === requestId) searchError = errorMessage(e);
    } finally {
      if (mine === requestId) loading = false;
    }
  }

  // Debounced search: wait for a pause in typing so the API is not hit on every keystroke.
  $effect(() => {
    const delay = text.trim() === '' ? 0 : 300;
    void text;
    const timer = setTimeout(() => search(true), delay);
    return () => clearTimeout(timer);
  });

  // ── Personal stations ──────────────────────────────────────────────────────────
  let personal = $state<PersonalStation[]>([]);
  const loadPersonal = async () => {
    try {
      personal = await api.listPersonalStations();
    } catch (e) {
      player.error = errorMessage(e);
    }
  };
  onMount(loadPersonal);

  async function removePersonal(station: PersonalStation) {
    try {
      await api.removePersonalStation(station.station_id);
      if (player.isFavorite(station.station_id)) await player.toggleFavorite(fromPersonal(station));
      await loadPersonal();
    } catch (e) {
      player.error = errorMessage(e);
    }
  }

  const matchingPersonal = $derived(
    text.trim() === ''
      ? []
      : personal.filter((p) => p.name.toLowerCase().includes(text.trim().toLowerCase())),
  );

  // ── Conversions to what the player can play ───────────────────────────
  const fromStation = (s: Station): Playable => ({ id: s.id, name: s.name, url: s.url, genre: s.tags[0] ?? null });
  const fromPersonal = (p: PersonalStation): Playable => ({ id: p.station_id, name: p.name, url: p.stream_url, genre: p.genre });
  const fromFavorite = (f: Favorite): Playable => ({ id: f.station_id, name: f.name, url: f.stream_url, genre: f.genre });

  const meta = (s: Station) =>
    [s.country_code, [s.codec, s.bitrate > 0 ? `${s.bitrate} kbit/s` : ''].filter(Boolean).join(' '), s.tags.slice(0, 2).join(', ')]
      .filter(Boolean)
      .join(' · ');

  function warning(s: Station): string | null {
    if (s.health.status !== 'unreachable') return null;
    const last = s.health.last_ok ? new Date(s.health.last_ok) : null;
    return last && !Number.isNaN(last.getTime())
      ? `Unreachable at last check (last success: ${last.toLocaleDateString(undefined, { dateStyle: 'medium' })})`
      : 'Unreachable at last check';
  }

  const isActive = (id: string) => player.current?.id === id;
</script>

<section class={styles.root} aria-label="Stations">
  <div class={styles.tabs} role="tablist">
    {#each TABS as t (t.id)}
      <button
        class={styles.tab}
        type="button"
        role="tab"
        id={`tab-${t.id}`}
        aria-selected={tab === t.id}
        aria-controls={`panel-${t.id}`}
        onclick={() => (tab = t.id)}
      >
        {t.label}
        {#if t.id === 'favorites' && player.favorites.length}<span class={styles.count}>{player.favorites.length}</span>{/if}
      </button>
    {/each}
  </div>

  <div class={styles.panel} role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
    {#if tab === 'history'}
      <HistoryPanel />
    {:else if tab === 'search'}
      <input
        class={styles.search}
        type="search"
        placeholder="Search stations by name…"
        aria-label="Search stations"
        bind:value={text}
        spellcheck="false"
        autocomplete="off"
      />

      <ul class={styles.list}>
        {#each matchingPersonal as p (p.station_id)}
          <StationRow
            name={p.name}
            personal
            active={isActive(p.station_id)}
            favorite={player.isFavorite(p.station_id)}
            onplay={() => player.play(fromPersonal(p))}
            onfavorite={() => player.toggleFavorite(fromPersonal(p))}
          />
        {/each}
        {#each results as s (s.id)}
          <StationRow
            name={s.name}
            meta={meta(s)}
            warning={warning(s)}
            active={isActive(s.id)}
            favorite={player.isFavorite(s.id)}
            onplay={() => player.play(fromStation(s))}
            onfavorite={() => player.toggleFavorite(fromStation(s))}
          />
        {/each}
      </ul>

      {#if searchError}
        <p class={styles.error} role="alert">{searchError}</p>
      {:else if loading && results.length === 0}
        <p class={styles.empty}>Searching…</p>
      {:else if !loading && results.length === 0}
        <p class={styles.empty}>No stations found.</p>
      {/if}

      {#if hasMore && !searchError}
        <button class={styles.more} type="button" disabled={loading} onclick={() => search(false)}>
          {loading ? 'Loading…' : 'Show more'}
        </button>
      {/if}
    {:else if tab === 'favorites'}
      {#if player.favorites.length === 0}
        <p class={styles.empty}>No favorites yet. Tap a station’s star to add it.</p>
      {:else}
        <ul class={styles.list}>
          {#each player.favorites as f (f.station_id)}
            <StationRow
              name={f.name}
              meta={f.genre ?? ''}
              personal={f.station_id.startsWith('user:')}
              active={isActive(f.station_id)}
              favorite
              onplay={() => player.play(fromFavorite(f))}
              onfavorite={() => player.toggleFavorite(fromFavorite(f))}
            />
          {/each}
        </ul>
      {/if}
    {:else}
      <AddStationForm onadded={loadPersonal} />
      {#if personal.length === 0}
        <p class={styles.empty}>No personal stations. They stay on this device.</p>
      {:else}
        <ul class={styles.list}>
          {#each personal as p (p.station_id)}
            <StationRow
              name={p.name}
              meta={p.genre ?? new URL(p.stream_url).host}
              personal
              active={isActive(p.station_id)}
              favorite={player.isFavorite(p.station_id)}
              onplay={() => player.play(fromPersonal(p))}
              onfavorite={() => player.toggleFavorite(fromPersonal(p))}
              onremove={() => removePersonal(p)}
            />
          {/each}
        </ul>
      {/if}
    {/if}
  </div>
</section>
