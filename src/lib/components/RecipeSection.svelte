<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- The station sheet's "Track on air": where the track on air comes from, and the recipe tooling to
     fix it when it is wrong or missing (edit, test, share, ask for one). -->
<script lang="ts">
  import {
    api,
    errorMessage,
    recipeRequestLink,
    type Playable,
    type RecipeTest,
    type SharedRecipe,
    type TitleStatus,
  } from '../api';
  import { player } from '../player.svelte';
  import { displayTitle } from '../trackLinks';
  import { ui } from '../ui.svelte';
  import styles from './StationSheet.module.css';

  let {
    station,
    status,
    open,
    error = $bindable(),
  }: {
    station: Playable;
    status: TitleStatus | null;
    /** Opens a link in the browser; a failure lands in `error`. */
    open: (url: string) => void;
    /** Shared with the sheet: a link of its details that fails to open says so here too. */
    error: string | null;
  } = $props();

  let test = $state<RecipeTest | null>(null);
  let testing = $state(false);
  let sharing = $state(false);
  let shared = $state<SharedRecipe | null>(null);
  let opened = $state<string | null>(null);
  const recipe = $derived(status?.recipe ?? null);

  // Each opening of the sheet starts without the results of the last one.
  $effect(() => {
    if (!ui.stationOpen) return;
    test = null;
    opened = null;
    shared = null;
  });

  // A saved recipe is reloaded by the app: an earlier test result no longer applies.
  $effect(() => {
    void player.recipeNotice;
    test = null;
  });

  const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

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

  async function shareRecipe() {
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
      <button
        class={styles.button}
        type="button"
        disabled={testing}
        aria-busy={testing}
        title={testing ? 'Reading the page…' : undefined}
        onclick={runTest}>Test</button
      >
    {/if}
    {#if recipe?.origin === 'local'}
      <button
        class={styles.button}
        type="button"
        disabled={sharing}
        aria-busy={sharing}
        title={sharing ? 'Reading the page…' : undefined}
        onclick={shareRecipe}>Share this recipe</button
      >
    {/if}
    {#if status?.state === 'none' && !recipe}
      <button class={styles.button} type="button" onclick={() => open(recipeRequestLink(station))}>
        Ask for a recipe
      </button>
    {/if}
  </div>
  {#if shared}
    <p class={styles.hint} role="status">
      GitHub is open with your recipe and an excerpt of the page filled in. Submit the form: a bot
      checks the recipe and proposes it for every listener.
      {#if shared.fixture_in_clipboard}
        <strong
          >The excerpt is too long for the link: it is in your clipboard, paste it into the Fixture
          field.</strong
        >
      {/if}
      A copy is in {shared.folder}.
    </p>
  {/if}
  {#if opened}
    <p class={styles.hint}>
      {fileName(opened)} is open in your editor. Save it and come back here: Soundfolk reloads it and
      reads the page again.
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
