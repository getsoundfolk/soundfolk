<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import {
    api,
    errorMessage,
    type Favorite,
    type PersonalStation,
    type Playable,
    type Station,
  } from '../api';
  import { rovingKeydown } from '../a11y';
  import { errors } from '../errors.svelte';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { Reorder } from '../reorder.svelte';
  import { pickRandom, playableOf, warningOf } from '../stations';
  import { stationNames } from '../stationNames.svelte';
  import { share } from '../share.svelte';
  import { ui } from '../ui.svelte';
  import AddStationForm from './AddStationForm.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import HistoryPanel from './HistoryPanel.svelte';
  import ListRow from './ListRow.svelte';
  import ListScreen from './ListScreen.svelte';
  import StationRow from './StationRow.svelte';
  import Icon from './Icon.svelte';
  import styles from './StationBrowser.module.css';
  import { updates } from '../updates.svelte';

  // The three destinations of docs/06-ui-principles.md §1: what is mine, what others offer, what I
  // heard.
  type Tab = 'listen' | 'discover' | 'history';
  const TABS: { id: Tab; label: string }[] = [
    { id: 'listen', label: 'Listen' },
    { id: 'discover', label: 'Discover' },
    { id: 'history', label: 'History' },
  ];
  const PAGE = 30;

  let tab = $state<Tab>('listen');

  // A list opens in the tab it was opened from: a favorite list in Listen, a Discover row in
  // Discover. From History (the station sheet's "In N lists"), it opens in Discover.
  let listTab = $state<Tab>('discover');
  $effect(() => {
    if (!ui.listRequest) return;
    untrack(() => {
      listTab = tab === 'listen' ? 'listen' : 'discover';
      tab = listTab;
    });
  });
  const openedList = $derived(ui.listId ? lists.get(ui.listId) : null);

  // Ctrl+F / Ctrl+K (`shortcuts.ts`): the search lives in Discover.
  let searchField = $state<HTMLInputElement>();
  $effect(() => {
    if (!ui.searchRequest) return;
    untrack(() => (tab = 'discover'));
    tick().then(() => searchField?.select());
  });

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
      if (mine === requestId) {
        searchError = errorMessage(e);
        searched = null;
      }
    } finally {
      if (mine === requestId) loading = false;
    }
  }

  // Text of the results shown, or null before the first search and after a failure (retried).
  let searched: string | null = null;

  // Debounced search: wait for a pause in typing so the API is not hit on every keystroke.
  // The catalog is only queried once text is typed in Discover, and not again for the same text;
  // without text, Discover shows its rows.
  $effect(() => {
    if (tab !== 'discover') return;
    const query = text.trim();
    if (query === searched) return;
    if (query === '') {
      ++requestId;
      searched = '';
      results = [];
      nextOffset = null;
      loading = false;
      searchError = null;
      return;
    }
    const timer = setTimeout(() => {
      searched = query;
      search(true);
    }, 300);
    return () => clearTimeout(timer);
  });

  // Typing: the matching lists first, as one row, then the stations (14 §7 D).
  const matchingLists = $derived(lists.matching(text));

  // ── Personal stations ──────────────────────────────────────────────────────────
  let personal = $state<PersonalStation[]>([]);
  let adding = $state(false);
  const loadPersonal = async () => {
    try {
      personal = await api.listPersonalStations();
    } catch (e) {
      errors.report('stations', e);
    }
  };
  onMount(loadPersonal);
  // Added from a share link (shared sheet): My stations reads them again.
  $effect(() => {
    if (ui.personalChanged) loadPersonal();
  });

  /** A share link pasted into the search opens what it carries instead of searching (16 §3.1). */
  async function pasteLink(e: ClipboardEvent) {
    const pasted = e.clipboardData?.getData('text/plain') ?? '';
    if (!share.mayBeLink(pasted)) return;
    e.preventDefault();
    if (await share.openText(pasted)) return;
    // Another web address: searched like any text.
    text = pasted.trim();
  }

  async function removePersonal(station: PersonalStation) {
    errors.clear('stations');
    try {
      await api.removePersonalStation(station.station_id);
      if (player.isFavorite(station.station_id)) await player.toggleFavorite(fromPersonal(station));
      await loadPersonal();
    } catch (e) {
      errors.report('stations', e);
    }
  }

  const matchingPersonal = $derived(
    text.trim() === ''
      ? []
      : personal.filter((p) =>
          [p.name, stationNames.shown(p.station_id, p.name)].some((n) =>
            n.toLowerCase().includes(text.trim().toLowerCase()),
          ),
        ),
  );

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
      listNotice = `${file.count} ${file.count === 1 ? 'station' : 'stations'} saved to Downloads: ${file.file_name}`;
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

  // ── Conversions to what the player can play ───────────────────────────
  const fromPersonal = (p: PersonalStation): Playable => ({
    id: p.station_id,
    name: p.name,
    url: p.stream_url,
    genre: p.genre,
  });
  const fromFavorite = (f: Favorite): Playable => ({
    id: f.station_id,
    name: f.name,
    url: f.stream_url,
    genre: f.genre,
  });

  const meta = (s: Station) =>
    [
      s.country_code,
      [s.codec, s.bitrate > 0 ? `${s.bitrate} kbit/s` : ''].filter(Boolean).join(' '),
      s.tags.slice(0, 2).join(', '),
    ]
      .filter(Boolean)
      .join(' · ');

  // Back from the settings: focus returns to the gear that opened them.
  let gear: HTMLButtonElement | undefined = $state();
  let wasOpen = false;
  $effect(() => {
    const open = ui.settingsOpen;
    if (wasOpen && !open) gear?.focus();
    wasOpen = open;
  });

  const stationsLabel = (n: number) => `${n} ${n === 1 ? 'station' : 'stations'}`;
  const isActive = (id: string) => player.current?.id === id;
  const favoriteCount = $derived(player.favorites.length + lists.favorites.length);
</script>

<section class={styles.root} aria-label="Stations">
  <!-- The tabs, then the window's own buttons at the end of the bar: they act on the app, not on
       the station on air, so they stay out of the player. -->
  <div class={styles.bar}>
    <div class={styles.tabs} role="tablist" tabindex="-1" onkeydown={rovingKeydown}>
      {#each TABS as t (t.id)}
        <button
          class={styles.tab}
          type="button"
          role="tab"
          id={`tab-${t.id}`}
          aria-selected={tab === t.id}
          tabindex={tab === t.id ? 0 : -1}
          aria-controls={`panel-${t.id}`}
          onclick={() => (tab = t.id)}
        >
          {t.label}
          {#if t.id === 'listen' && favoriteCount}<span class={styles.count}>{favoriteCount}</span
            >{/if}
        </button>
      {/each}
    </div>
    <button
      class={styles.tool}
      type="button"
      aria-label="Mini player"
      title="Mini player"
      onclick={() => api.openMiniPlayer().catch((e) => errors.report('player', e))}
      ><Icon name="mini" size={18} /></button
    >
    <button
      class={styles.tool}
      type="button"
      aria-label={updates.available ? 'Settings (a new version is available)' : 'Settings'}
      title={updates.available
        ? `Settings · Soundfolk ${updates.available.version} is out`
        : 'Settings'}
      bind:this={gear}
      onclick={ui.openSettings}
      ><Icon name="settings" size={18} />{#if updates.available}<span
          class={styles.dot}
          aria-hidden="true"
        ></span>{/if}</button
    >
  </div>

  <div class={styles.panel} role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
    {#if tab === 'history'}
      <HistoryPanel />
    {:else if tab === 'listen' && openedList && listTab === 'listen'}
      <ListScreen list={openedList} back="Listen" />
    {:else if tab === 'listen'}
      {#if listNotice}<p class={styles.notice} role="status">{listNotice}</p>{/if}
      {#if listError}<p class={styles.error} role="alert">{listError}</p>{/if}

      <section class={styles.section} aria-labelledby="favorites-title">
        <header class={styles.sectionHeader}>
          <h2 class={styles.sectionTitle} id="favorites-title">Favorites</h2>
          {#if player.favorites.length}
            <button
              class={styles.link}
              type="button"
              onclick={() => surpriseFrom(player.favorites.map(fromFavorite))}>Surprise me</button
            >
            <button class={styles.link} type="button" onclick={() => exportMine('favorites')}
              >Export M3U</button
            >
          {/if}
        </header>
        {#if favoriteCount === 0}
          <p class={styles.hint}>
            Star a station or a list to keep it here.
            <button class={styles.link} type="button" onclick={() => (tab = 'discover')}
              >Discover stations</button
            >
          </p>
        {/if}
        {#if lists.favorites.length}
          <ul class={styles.cards}>
            {#each lists.favorites as l (l.id)}
              <li class={styles.card}>
                <button class={styles.cardOpen} type="button" onclick={() => ui.openList(l.id)}>
                  <span class={styles.cardTitle}>{l.title}</span>
                  <span class={styles.cardMeta}
                    >List by {l.curators.map((c) => `@${c}`).join(', ')} · {stationsLabel(
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
                  onclick={() =>
                    lists.toggleFavorite(l.id).catch((e) => errors.report('player', e))}
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
                meta={f.genre ?? ''}
                personal={f.station_id.startsWith('user:')}
                active={isActive(f.station_id)}
                favorite
                onplay={() => player.play(fromFavorite(f))}
                onfavorite={() => player.toggleFavorite(fromFavorite(f))}
                handle={player.favorites.length > 1
                  ? favoriteOrder.handle(f.station_id)
                  : undefined}
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
              onclick={() => surpriseFrom(personal.map(fromPersonal))}>Surprise me</button
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
            >{#if !adding}<Icon name="plus" size={14} />{/if}{adding
              ? 'Cancel'
              : 'Add a station'}</button
          >
        </header>
        <ErrorBanner area="stations" />
        {#if adding}
          <AddStationForm
            onadded={() => {
              adding = false;
              loadPersonal();
            }}
          />
        {/if}
        {#if personal.length === 0 && !adding}
          <p class={styles.hint}>
            Stations you add by their stream address. They stay on this device.
          </p>
        {:else}
          <ul class={styles.list}>
            {#each personal as p (p.station_id)}
              <StationRow
                name={stationNames.shown(p.station_id, p.name)}
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
      </section>
    {:else if openedList && listTab === 'discover'}
      <ListScreen list={openedList} back="Discover" />
    {:else}
      <div class={styles.searchRow}>
        <input
          class={styles.search}
          type="search"
          bind:this={searchField}
          placeholder="Search stations and lists"
          aria-label="Search stations and lists"
          bind:value={text}
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

      {#if text.trim() === ''}
        {#each lists.all as l (l.id)}
          <ListRow list={l} />
        {:else}
          <p class={styles.empty}>
            No lists yet. They are downloaded once a day from soundfolk-stations; type a name to
            search the stations.
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
                  <span class={styles.cardMeta}>List · {stationsLabel(l.items.length)}</span>
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
              onplay={() => player.play(fromPersonal(p))}
              onfavorite={() => player.toggleFavorite(fromPersonal(p))}
            />
          {/each}
          {#each results as s (s.id)}
            <StationRow
              name={stationNames.shown(s.id, s.name)}
              meta={meta(s)}
              warning={warningOf(s)}
              active={isActive(s.id)}
              favorite={player.isFavorite(s.id)}
              onplay={() => player.play(playableOf(s))}
              onfavorite={() => player.toggleFavorite(playableOf(s))}
              oninfo={() => ui.showStation(s)}
            />
          {/each}
        </ul>

        {#if searchError}
          <p class={styles.error} role="alert">{searchError}</p>
        {:else if loading && results.length === 0}
          <p class={styles.empty}>Searching…</p>
        {:else if !loading && results.length === 0 && matchingPersonal.length === 0}
          <p class={styles.empty}>No stations found.</p>
        {/if}

        {#if hasMore && !searchError}
          <button
            class={styles.more}
            type="button"
            disabled={loading}
            aria-busy={loading}
            title={loading ? 'Loading…' : undefined}
            onclick={() => search(false)}>Show more</button
          >
        {/if}
      {/if}
    {/if}
  </div>
</section>
