<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Details of a station, on demand (docs/09-search-filters.md §4.2): from a list, to know it before
     playing or liking it; from the player, also where its track on air comes from, with the way to
     fix it (a local recipe) when it is wrong or missing. -->
<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorMessage, type ListRef, type Station } from '../api';
  import { errors } from '../errors.svelte';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { share } from '../share.svelte';
  import { stationNames } from '../stationNames.svelte';
  import { isCatalogId, playableOf, quality, warningOf } from '../stations';
  import { ui } from '../ui.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import Icon from './Icon.svelte';
  import RecipeSection from './RecipeSection.svelte';
  import styles from './StationSheet.module.css';

  let dialog: HTMLDialogElement | undefined = $state();
  /** Whether the last input was the mouse (or a touch), not the keyboard: opening the sheet focuses
   *  its first button, closing it gives focus back to the button that opened it, and WebKitGTK
   *  rings both as if tabbed to. */
  let byPointer = false;
  let error = $state<string | null>(null);
  /** Gain Constant volume applies to the station, in dB (null when off or not playing). */
  let level = $state<number | null>(null);

  /** The station in the player as the catalog has it, read when the sheet opens from the player. */
  let fetched = $state<Station | null>(null);
  /** The catalog station shown, with its country, quality…: the one opened from a list, or the
   *  one in the player once read from the catalog; null for a personal station. */
  const catalog = $derived(
    ui.sheetStation ?? (fetched && fetched.id === player.current?.id ? fetched : null),
  );
  const station = $derived(catalog ?? player.current);
  /** The parts tied to playback (track on air, recipes, volume) show only for the station playing. */
  const playing = $derived(!!station && player.current?.id === station.id);
  const status = $derived(playing ? player.titleStatus : null);
  /** Lists of a station from a list that is not playing (the player's are in `lists.forCurrent`). */
  let catalogLists = $state<ListRef[]>([]);
  const inLists = $derived(playing ? lists.forCurrent : catalogLists);
  /** The name the user gave the station, if any; the station keeps its own as `station.name`. */
  const given = $derived(station ? stationNames.given(station.id) : null);
  let nameError = $state<string | null>(null);

  $effect(() => {
    if (!dialog) return;
    if (ui.stationOpen && !dialog.open) {
      error = null;
      nameError = null;
      dialog.showModal();
      // Opening focuses its first button: ringed in WebKitGTK, even from a click. The sheet itself
      // takes the focus instead (a blur only sends WebKitGTK to the next button).
      if (byPointer) dialog.focus();
    } else if (!ui.stationOpen && dialog.open) {
      dialog.close();
    }
  });

  // Opened from the player: the same details as from a list, once the catalog answers.
  $effect(() => {
    const id = player.current?.id;
    if (!ui.stationOpen || ui.sheetStation || !isCatalogId(id) || fetched?.id === id) return;
    let current = true;
    api
      .getStation(id)
      .then((found) => {
        if (current) fetched = found;
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  });

  $effect(() => {
    catalogLists = [];
    if (!catalog || playing) return;
    const asked = catalog;
    api
      .getListsForStation(asked.id, asked.url)
      .then((refs) => {
        if (ui.sheetStation === asked) catalogLists = refs;
      })
      .catch(() => {});
  });

  // The gain moves while the station plays: read again every few seconds while the sheet is open.
  $effect(() => {
    if (!ui.stationOpen || !playing) return;
    const read = () =>
      api
        .getLevelAdjustment()
        .then((db) => (level = db))
        .catch(() => (level = null));
    read();
    const timer = setInterval(read, 2000);
    return () => clearInterval(timer);
  });

  /** The catalog's last check of the stream, in words. */
  function health(s: Station): string {
    switch (s.health.status) {
      case 'ok':
        return 'Reachable at last check';
      case 'unsupported':
        return `Not playable here: ${s.health.reason}`;
      case 'unreachable':
        return warningOf(s) ?? 'Unreachable at last check';
    }
  }

  const host = (url: string) => url.replace(/^https?:\/\//, '').replace(/\/$/, '');

  /** Opens a link in the browser; a failure shows under the sheet's actions. */
  function open(url: string) {
    error = null;
    openUrl(url).catch((e) => (error = errorMessage(e)));
  }

  /** Saves the name typed (on Enter or when the field loses focus). Its own name, or a blank
   *  field, gives the station back its own name. */
  async function rename(input: HTMLInputElement) {
    if (!station) return;
    nameError = null;
    try {
      const typed = input.value.trim();
      await stationNames.rename(station.id, typed === station.name.trim() ? '' : typed);
    } catch (e) {
      nameError = errorMessage(e);
    }
    input.value = stationNames.given(station.id) ?? station.name;
  }
</script>

<svelte:window
  onpointerdowncapture={() => (byPointer = true)}
  onkeydowncapture={() => (byPointer = false)}
/>

<dialog
  class={styles.sheet}
  bind:this={dialog}
  tabindex="-1"
  aria-labelledby="station-sheet-title"
  onclose={() => {
    // Seen in the sheet: not shown again in the window.
    player.recipeNotice = null;
    // The sender and track of a link belong to the visit it opened.
    share.note = null;
    ui.closeStation();
    // Opened and closed with the mouse: no focus ring left on the button it came back to.
    if (byPointer && document.activeElement instanceof HTMLElement) document.activeElement.blur();
  }}
  onclick={(e) => e.target === dialog && ui.closeStation()}
>
  {#if station}
    <header class={styles.header}>
      <h2 class={styles.title} id="station-sheet-title">{given ?? station.name}</h2>
      <div class={styles.headerActions}>
        {#if catalog}
          {@const favorite = player.isFavorite(catalog.id)}
          {#if !playing}
            <button
              class={styles.primary}
              type="button"
              onclick={() => player.play(playableOf(catalog))}
              ><Icon name="play" size={14} /> Play</button
            >
          {/if}
          <button
            class={styles.close}
            type="button"
            aria-pressed={favorite}
            aria-label={favorite ? 'Remove from favorites' : 'Add to favorites'}
            title={favorite ? 'Remove from favorites' : 'Add to favorites'}
            data-on={favorite}
            onclick={() => player.toggleFavorite(playableOf(catalog))}
            ><Icon name="star" filled={favorite} /></button
          >
        {/if}
        <!-- The track on air goes with the station playing: it is what makes a link clicked
             (docs/16-sharing.md §4.1). -->
        <button
          class={styles.close}
          type="button"
          aria-label={`Share ${given ?? station.name}`}
          title="Share"
          disabled={share.busy}
          onclick={() =>
            share.offerStation(
              catalog ? playableOf(catalog) : station,
              playing ? (player.nowPlaying.title ?? null) : null,
            )}><Icon name="share" /></button
        >
      </div>
      <button class={styles.close} type="button" aria-label="Close" onclick={ui.closeStation}
        ><Icon name="x" /></button
      >
    </header>

    <!-- Play and ★ report in the player, which the modal sheet covers. -->
    {#if errors.player}<div class={styles.section}><ErrorBanner area="player" /></div>{/if}

    {#if share.note && share.note.stationId === station.id && (share.note.from || share.note.track)}
      {@const note = share.note}
      <!-- From a link: who sent it (as they typed it, not verified) and what was on air then. -->
      <div class={styles.section}>
        {#if note.from}<p class={styles.text}>From {note.from}</p>{/if}
        {#if note.track}
          <p class={styles.text}>
            {note.from ? `${note.from} was listening to` : 'On air when the link was made:'}
            {note.track}
          </p>
        {/if}
      </div>
    {/if}

    <dl class={styles.details}>
      <div>
        <dt><label for="station-sheet-name">Name</label></dt>
        <dd>
          <input
            id="station-sheet-name"
            class={styles.input}
            type="text"
            value={given ?? station.name}
            aria-describedby="station-sheet-name-hint"
            spellcheck="false"
            autocomplete="off"
            onkeydown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur();
              // Escape closes the sheet: the name typed is dropped, not saved.
              else if (e.key === 'Escape') e.currentTarget.value = given ?? station.name;
            }}
            onchange={(e) => rename(e.currentTarget)}
          />
          <p class={styles.hint} id="station-sheet-name-hint">
            {#if given}
              Shown instead of its own name, {station.name}.
              <button
                class={styles.inline}
                type="button"
                onclick={() =>
                  stationNames.rename(station.id, '').catch((e) => (nameError = errorMessage(e)))}
                >Use its own name</button
              >
            {:else}
              Change it to see this station under another name.
            {/if}
          </p>
          {#if nameError}<p class={styles.warning} role="alert">{nameError}</p>{/if}
        </dd>
      </div>
      {#if catalog}
        <!-- No genres until the tags map to translated genres (§3.1): raw tags are in the technical details. -->
        {#if catalog.country || catalog.country_code}
          <div>
            <dt>Country</dt>
            <dd>{catalog.country ?? catalog.country_code}</dd>
          </div>
        {/if}
        {#if catalog.language}
          <div>
            <dt>Language</dt>
            <dd>{catalog.language}</dd>
          </div>
        {/if}
        {#if quality(catalog)}
          <div>
            <dt>Quality</dt>
            <dd>{quality(catalog)}</dd>
          </div>
        {/if}
        {#if catalog.health.status !== 'ok'}
          <div>
            <dt>Status</dt>
            <dd class={styles.warning}>{health(catalog)}</dd>
          </div>
        {/if}
      {/if}
      {#if station.homepage}
        <div>
          <dt>Website</dt>
          <dd>
            <button class={styles.link} type="button" onclick={() => open(station.homepage!)}>
              {host(station.homepage)}
            </button>
          </dd>
        </div>
      {/if}
      <!-- In sight: two copies of a station often differ only by where their stream points. -->
      <div>
        <dt>Stream</dt>
        <dd class={styles.mono}>{station.url}</dd>
      </div>
      {#if level !== null}
        <div>
          <dt>Volume</dt>
          <dd>
            {level === 0
              ? 'Level not adjusted'
              : `Level adjusted: ${level > 0 ? '+' : '−'}${Math.abs(level)} dB`}
          </dd>
        </div>
      {/if}
    </dl>

    {#if catalog}
      <details class={styles.technical}>
        <summary>Technical details</summary>
        <dl class={styles.details}>
          {#if catalog.tags.length}
            <div>
              <dt>Tags</dt>
              <dd class={styles.mono}>{catalog.tags.join(', ')}</dd>
            </div>
          {/if}
          <div>
            <dt>Votes</dt>
            <dd>{catalog.votes.toLocaleString()}</dd>
          </div>
          <div>
            <dt>Last check</dt>
            <dd>{health(catalog)}</dd>
          </div>
          <div>
            <dt>Catalog id</dt>
            <dd class={styles.mono}>
              {#if isCatalogId(catalog.id)}
                <!-- The station's page on Radio-Browser: its history of changes, clicks and checks. -->
                <button
                  class={styles.link}
                  type="button"
                  title="Open on Radio-Browser"
                  onclick={() => open(`https://www.radio-browser.info/history/${catalog.id}`)}
                  >{catalog.id}</button
                >
              {:else}
                {catalog.id}
              {/if}
            </dd>
          </div>
        </dl>
      </details>
      <!-- Without the track section (station not playing), a link that fails to open says so here. -->
      {#if error && !playing}<p class={styles.alert} role="alert">{error}</p>{/if}
    {/if}

    {#if inLists.length}
      <section class={styles.section} aria-labelledby="station-sheet-lists">
        <h3 class={styles.sectionTitle} id="station-sheet-lists">In these lists</h3>
        <div class={styles.actions}>
          {#each inLists as list (list.id)}
            <button class={styles.chip} type="button" onclick={() => ui.openList(list.id)}
              >{list.title}</button
            >
          {/each}
        </div>
      </section>
    {/if}

    {#if playing || !catalog}
      <RecipeSection {station} {status} {open} bind:error />
    {/if}
  {/if}
</dialog>
