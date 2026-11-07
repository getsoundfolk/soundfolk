<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Details of a station, on demand (docs/09-search-filters.md §4.2): from a list, to know it before
     playing or liking it; from the player, also where its track on air comes from, with the way to
     fix it (a local recipe) when it is wrong or missing. -->
<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import {
    api,
    errorMessage,
    recipeRequestLink,
    type ListRef,
    type RecipeTest,
    type SharedRecipe,
    type Station,
    type TitleStatus,
  } from '../api';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { stationNames } from '../stationNames.svelte';
  import { playableOf } from '../stations';
  import { displayTitle } from '../trackLinks';
  import { ui } from '../ui.svelte';
  import Icon from './Icon.svelte';
  import styles from './StationSheet.module.css';

  let dialog: HTMLDialogElement | undefined = $state();
  let test = $state<RecipeTest | null>(null);
  let testing = $state(false);
  let sharing = $state(false);
  let shared = $state<SharedRecipe | null>(null);
  let opened = $state<string | null>(null);
  let error = $state<string | null>(null);
  /** Gain Constant volume applies to the station, in dB (null when off or not playing). */
  let level = $state<number | null>(null);

  /** The catalog station opened from a list, with its genres, country, quality…; null from the player. */
  const catalog = $derived(ui.sheetStation);
  const station = $derived(catalog ?? player.current);
  /** The parts tied to playback (track on air, recipes, volume) show only for the station playing. */
  const playing = $derived(!!station && player.current?.id === station.id);
  const status = $derived(playing ? player.titleStatus : null);
  /** Lists of a station from a list that is not playing (the player's are in `lists.forCurrent`). */
  let catalogLists = $state<ListRef[]>([]);
  const inLists = $derived(playing ? lists.forCurrent : catalogLists);
  const recipe = $derived(status?.recipe ?? null);
  /** The name the user gave the station, if any; the station keeps its own as `station.name`. */
  const given = $derived(station ? stationNames.given(station.id) : null);
  let nameError = $state<string | null>(null);

  $effect(() => {
    if (!dialog) return;
    if (ui.stationOpen && !dialog.open) {
      test = null;
      opened = null;
      shared = null;
      error = null;
      nameError = null;
      dialog.showModal();
    } else if (!ui.stationOpen && dialog.open) {
      dialog.close();
    }
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

  // A saved recipe is reloaded by the app: an earlier test result no longer applies.
  $effect(() => {
    void player.recipeNotice;
    test = null;
  });

  const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

  /** Codecs that sound as good as MP3 at about half the bitrate. */
  const EFFICIENT = /aac|opus|ogg|vorbis/i;
  const LOSSLESS = /flac|alac|wav/i;

  /** "Good quality — AAC 128 kbit/s": the quality in words, then the figures the catalog gives. */
  function quality(s: Station): string | null {
    const codec = s.codec?.trim() || null;
    const figures = [codec, s.bitrate > 0 ? `${s.bitrate} kbit/s` : null].filter(Boolean).join(' ');
    let words: string | null = null;
    if (codec && LOSSLESS.test(codec)) words = 'Lossless';
    else if (s.bitrate > 0) {
      const mp3 = codec && EFFICIENT.test(codec) ? s.bitrate * 2 : s.bitrate;
      words = mp3 >= 192 ? 'High quality' : mp3 >= 128 ? 'Good quality' : 'Low quality';
    }
    if (words && figures) return `${words} — ${figures}`;
    return words ?? (figures || null);
  }

  /** The catalog's last check of the stream, in words. */
  function health(s: Station): string {
    switch (s.health.status) {
      case 'ok':
        return 'Reachable at last check';
      case 'unsupported':
        return `Not playable here: ${s.health.reason}`;
      case 'unreachable': {
        const last = s.health.last_ok ? new Date(s.health.last_ok) : null;
        return last && !Number.isNaN(last.getTime())
          ? `Unreachable at last check (last success: ${last.toLocaleDateString(undefined, { dateStyle: 'medium' })})`
          : 'Unreachable at last check';
      }
    }
  }

  const host = (url: string) => url.replace(/^https?:\/\//, '').replace(/\/$/, '');

  function describe(status: TitleStatus | null): string {
    switch (status?.state ?? 'idle') {
      case 'idle':
        return 'Play the station to see where its track on air comes from.';
      case 'off':
        return 'Reading station websites is turned off in the settings (Privacy).';
      case 'searching':
        return 'Looking for it…';
      case 'stream':
        return status?.adapter
          ? `Announced by the stream itself; artist, title and next track from the streaming server (${status.adapter}).`
          : 'Announced by the stream itself.';
      case 'recipe':
        switch (status?.recipe?.origin) {
          case 'local':
            return `Read from the station's website with your recipe ${fileName(status.recipe.path ?? '')}.`;
          case 'shared':
            return `Read from the station's website with the shared recipe “${status.recipe.name}”.`;
          default:
            return `Read from the station's website with the recipe “${status?.recipe?.name}” shipped with Soundfolk.`;
        }
      case 'server':
        return `Read from the streaming server's status page (${status?.adapter}).`;
      case 'none':
        return "Not available: the stream doesn't announce it, and no recipe reads it from the station's website.";
    }
  }

  const time = (iso: string | null) => {
    if (!iso) return null;
    const d = new Date(iso);
    return Number.isNaN(d.getTime())
      ? null
      : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  };
  const utc = (minutes: number) => {
    const sign = minutes < 0 ? '−' : '+';
    const abs = Math.abs(minutes);
    return `UTC${sign}${Math.floor(abs / 60)}${abs % 60 ? `:${String(abs % 60).padStart(2, '0')}` : ''}`;
  };

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

  async function edit() {
    const station = player.current;
    if (!station) return;
    error = null;
    try {
      opened = await api.editRecipe(station);
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function share() {
    const station = player.current;
    if (!station) return;
    sharing = true;
    error = null;
    shared = null;
    try {
      shared = await api.shareRecipe(station.url, station.id);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      sharing = false;
    }
  }

  async function runTest() {
    const station = player.current;
    if (!station) return;
    testing = true;
    error = null;
    try {
      test = await api.testRecipe(station.url, station.id);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      testing = false;
    }
  }
</script>

<dialog
  class={styles.sheet}
  bind:this={dialog}
  aria-labelledby="station-sheet-title"
  onclose={() => {
    // Seen in the sheet: not shown again in the window.
    player.recipeNotice = null;
    ui.closeStation();
  }}
  onclick={(e) => e.target === dialog && ui.closeStation()}
>
  {#if station}
    <header class={styles.header}>
      <h2 class={styles.title} id="station-sheet-title">{given ?? station.name}</h2>
      {#if catalog}
        {@const favorite = player.isFavorite(catalog.id)}
        <div class={styles.headerActions}>
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
        </div>
      {/if}
      <button class={styles.close} type="button" aria-label="Close" onclick={ui.closeStation}
        ><Icon name="x" /></button
      >
    </header>

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
        {#if catalog.tags.length}
          <div>
            <dt>Genres</dt>
            <dd>{catalog.tags.slice(0, 4).join(', ')}</dd>
          </div>
        {/if}
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
      {#if !catalog}
        <div>
          <dt>Stream</dt>
          <dd class={styles.mono}>{station.url}</dd>
        </div>
      {/if}
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
          <div>
            <dt>Stream</dt>
            <dd class={styles.mono}>{catalog.url}</dd>
          </div>
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
            <dd class={styles.mono}>{catalog.id}</dd>
          </div>
        </dl>
      </details>
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
      <section class={styles.section} aria-labelledby="station-sheet-track">
        <h3 class={styles.sectionTitle} id="station-sheet-track">Track on air</h3>
        <p class={styles.text}>{describe(status)}</p>
        {#if status?.error}<p class={styles.warning}>{status.error}</p>{/if}
        {#if status?.local_error}
          <p class={styles.warning}>
            Your recipe {fileName(status.local_error.path)} isn't used: {status.local_error.error}
          </p>
        {/if}

        <div class={styles.actions}>
          <!-- No new recipe where the stream announces its titles and the server's API completes them. -->
          {#if recipe || status?.local_error || !(status?.state === 'stream' && status.adapter)}
            <button class={styles.button} type="button" onclick={edit}>
              {recipe || status?.local_error ? 'Edit locally' : 'Create a recipe'}
            </button>
          {/if}
          {#if recipe}
            <button class={styles.button} type="button" disabled={testing} onclick={runTest}>
              {testing ? 'Reading the page…' : 'Test'}
            </button>
          {/if}
          {#if recipe?.origin === 'local'}
            <button class={styles.button} type="button" disabled={sharing} onclick={share}>
              {sharing ? 'Reading the page…' : 'Share this recipe'}
            </button>
          {/if}
          {#if status?.state === 'none' && !recipe}
            <button
              class={styles.button}
              type="button"
              onclick={() => open(recipeRequestLink(station))}
            >
              Ask for a recipe
            </button>
          {/if}
        </div>
        {#if shared}
          <p class={styles.hint} role="status">
            GitHub is open with your recipe and an excerpt of the page filled in. Submit the form: a
            bot checks the recipe and proposes it for every listener.
            {#if shared.fixture_in_clipboard}
              <strong
                >The excerpt is too long for the link: it is in your clipboard, paste it into the
                Fixture field.</strong
              >
            {/if}
            A copy is in {shared.folder}.
          </p>
        {/if}
        {#if opened}
          <p class={styles.hint}>
            {fileName(opened)} is open in your editor. Save it and come back here: Soundfolk reloads it
            and reads the page again.
          </p>
        {/if}
        {#if error}<p class={styles.warning} role="alert">{error}</p>{/if}
        <!-- The window's notice stays behind the open sheet: shown here instead. -->
        {#if player.recipeNotice}
          {@const notice = player.recipeNotice}
          <p class={notice.ok ? styles.saved : styles.warning} role="status">
            Saved {notice.file}: {notice.message}
          </p>
        {/if}

        {#if test}
          <div class={styles.result} role="status">
            {#if test.track}
              <p><span class={styles.label}>Reads</span> {displayTitle(test.track)}</p>
              {#if time(test.track.started_at)}
                <p><span class={styles.label}>Started</span> {time(test.track.started_at)}</p>
              {/if}
              {#if test.next}
                <p>
                  <span class={styles.label}>Next</span>
                  {displayTitle(test.next)}{#if time(test.next.started_at)}&nbsp;at {time(
                      test.next.started_at,
                    )}{/if}
                </p>
              {/if}
              {#if test.show}<p><span class={styles.label}>Show</span> {test.show}</p>{/if}
              {#if test.utc_offset_minutes !== null}
                <p><span class={styles.label}>Page time</span> {utc(test.utc_offset_minutes)}</p>
              {/if}
            {/if}
            {#if test.error}<p class={styles.warning}>{test.error}</p>{/if}
          </div>
        {/if}
      </section>
    {/if}
  {/if}
</dialog>
