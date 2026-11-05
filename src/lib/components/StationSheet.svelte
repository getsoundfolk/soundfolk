<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Details of the station in the player, on demand: its links, and where its track on air comes
     from, with the way to fix it (a local recipe) when it is wrong or missing. -->
<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorMessage, type RecipeTest, type TitleStatus, type TrackInfo } from '../api';
  import { player } from '../player.svelte';
  import { ui } from '../ui.svelte';
  import styles from './StationSheet.module.css';

  let dialog: HTMLDialogElement | undefined = $state();
  let test = $state<RecipeTest | null>(null);
  let testing = $state(false);
  let opened = $state<string | null>(null);
  let error = $state<string | null>(null);

  const station = $derived(player.current);
  const status = $derived(player.titleStatus);
  const recipe = $derived(status?.recipe ?? null);

  $effect(() => {
    if (!dialog) return;
    if (ui.stationOpen && !dialog.open) {
      test = null;
      opened = null;
      error = null;
      dialog.showModal();
    } else if (!ui.stationOpen && dialog.open) {
      dialog.close();
    }
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
        return "Reading station websites is turned off in the settings (Privacy).";
      case 'searching':
        return 'Looking for it…';
      case 'stream':
        return 'Announced by the stream itself.';
      case 'recipe':
        return status?.recipe?.origin === 'local'
          ? `Read from the station's website with your recipe ${fileName(status.recipe.path ?? '')}.`
          : `Read from the station's website with the recipe “${status?.recipe?.name}” shipped with Soundfolk.`;
      case 'server':
        return `Read from the streaming server's status page (${status?.adapter}).`;
      case 'none':
        return "Not available: the stream doesn't announce it, and no recipe reads it from the station's website.";
    }
  }

  const trackText = (t: TrackInfo) => (t.artist ? `${t.artist} – ${t.title}` : t.title);
  const time = (iso: string | null) => {
    if (!iso) return null;
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? null : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  };
  const utc = (minutes: number) => {
    const sign = minutes < 0 ? '−' : '+';
    const abs = Math.abs(minutes);
    return `UTC${sign}${Math.floor(abs / 60)}${abs % 60 ? `:${String(abs % 60).padStart(2, '0')}` : ''}`;
  };

  async function edit() {
    if (!station) return;
    error = null;
    try {
      opened = await api.editRecipe(station);
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function runTest() {
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
      <h2 class={styles.title} id="station-sheet-title">{station.name}</h2>
      <button class={styles.close} type="button" aria-label="Close" onclick={ui.closeStation}>✕</button>
    </header>

    <dl class={styles.details}>
      {#if station.homepage}
        <div>
          <dt>Website</dt>
          <dd>
            <button class={styles.link} type="button" onclick={() => openUrl(station.homepage!)}>
              {station.homepage.replace(/^https?:\/\//, '').replace(/\/$/, '')}
            </button>
          </dd>
        </div>
      {/if}
      <div>
        <dt>Stream</dt>
        <dd class={styles.mono}>{station.url}</dd>
      </div>
    </dl>

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
        <button class={styles.button} type="button" onclick={edit}>
          {recipe || status?.local_error ? 'Edit locally' : 'Create a recipe'}
        </button>
        {#if recipe}
          <button class={styles.button} type="button" disabled={testing} onclick={runTest}>
            {testing ? 'Reading the page…' : 'Test'}
          </button>
        {/if}
      </div>
      {#if opened}
        <p class={styles.hint}>
          {fileName(opened)} is open in your editor. Save it and come back here: Soundfolk reloads it and reads the
          page again.
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
            <p><span class={styles.label}>Reads</span> {trackText(test.track)}</p>
            {#if time(test.track.started_at)}
              <p><span class={styles.label}>Started</span> {time(test.track.started_at)}</p>
            {/if}
            {#if test.next}
              <p>
                <span class={styles.label}>Next</span>
                {trackText(test.next)}{#if time(test.next.started_at)}&nbsp;at {time(test.next.started_at)}{/if}
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
</dialog>
