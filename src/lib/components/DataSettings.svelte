<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Settings › Your data: export or clear the listening history. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type HistorySummary } from '../api';
  import { errors } from '../errors.svelte';
  import { history } from '../history.svelte';
  import styles from './SettingsPage.module.css';

  let {
    confirmingClear = $bindable(false),
  }: {
    /** The deletion waits for its confirmation; the page's Escape cancels it. */
    confirmingClear?: boolean;
  } = $props();

  let summary = $state<HistorySummary | null>(null);
  let exporting = $state(false);
  let clearing = $state(false);
  let dataStatus = $state<string | null>(null);
  let cancelClear: HTMLButtonElement | undefined = $state();

  const tracks = (n: number) => `${n.toLocaleString()} ${n === 1 ? 'track' : 'tracks'}`;
  const since = $derived(
    summary?.since
      ? new Date(`${summary.since.replace(' ', 'T')}Z`).toLocaleDateString(undefined, {
          dateStyle: 'long',
        })
      : null,
  );

  onMount(() => {
    loadSummary();
  });

  async function loadSummary() {
    try {
      summary = await api.getHistorySummary();
    } catch (e) {
      errors.report('settings', e);
    }
  }

  async function exportHistory(format: 'csv' | 'json') {
    if (exporting) return;
    exporting = true;
    dataStatus = null;
    try {
      const file = await api.exportHistory('history', format, null);
      dataStatus = `${tracks(file.count)} saved to Downloads: ${file.file_name}`;
    } catch (e) {
      errors.report('settings', e);
    } finally {
      exporting = false;
    }
  }

  function askClear() {
    confirmingClear = true;
    dataStatus = null;
    // The safe answer has the focus: Enter does not delete.
    queueMicrotask(() => cancelClear?.focus());
  }

  async function clearHistory() {
    if (clearing) return;
    clearing = true;
    try {
      const removed = await api.clearListeningHistory();
      history.cleared();
      dataStatus = `${tracks(removed)} deleted.`;
      confirmingClear = false;
      await loadSummary();
    } catch (e) {
      errors.report('settings', e);
    } finally {
      clearing = false;
    }
  }
</script>

<section class={styles.group} aria-labelledby="settings-data">
  <h2 class={styles.groupTitle} id="settings-data">Your data</h2>
  <p class={styles.groupIntro}>
    Your listening history is kept on this device only. It feeds the History tab and the stats.
  </p>

  <div class={styles.dataRow}>
    <span class={styles.text}>
      <span class={styles.name}>Export my history</span>
      <span class={styles.detail}>
        Every track heard, with its station and time, as a file in your Downloads folder. Liked
        tracks alone can be exported from History › Liked.
      </span>
    </span>
    <span class={styles.dataActions}>
      <button
        class={styles.link}
        type="button"
        disabled={exporting || !summary?.count}
        onclick={() => exportHistory('csv')}>CSV</button
      >
      <button
        class={styles.link}
        type="button"
        disabled={exporting || !summary?.count}
        onclick={() => exportHistory('json')}>JSON</button
      >
    </span>
  </div>

  <div class={styles.dataRow} data-danger={confirmingClear}>
    <span class={styles.text}>
      <span class={styles.name}>Clear my history</span>
      <span class={styles.detail}>
        {#if !summary}
          …
        {:else if summary.count === 0}
          The history is empty.
        {:else}
          {tracks(summary.count)}{since ? ` since ${since}` : ''}. Liked tracks are kept.
        {/if}
      </span>
      {#if confirmingClear && summary?.count}
        <span class={styles.warning} role="alert">
          This deletes for good: {tracks(summary.count)}, the listening time of every station (all
          the stats start over), and the volume Constant volume learned for each station. Liked
          tracks are kept. This cannot be undone: export first if you may want the tracks back.
        </span>
        <span class={styles.formActions}>
          <button
            class={styles.link}
            type="button"
            bind:this={cancelClear}
            onclick={() => (confirmingClear = false)}>Cancel</button
          >
          <button class={styles.danger} type="button" disabled={clearing} onclick={clearHistory}>
            Delete {tracks(summary.count)}
          </button>
        </span>
      {/if}
    </span>
    {#if !confirmingClear}
      <button
        class="{styles.link} {styles.dataActions}"
        type="button"
        disabled={!summary?.count}
        onclick={askClear}>Clear…</button
      >
    {/if}
  </div>

  {#if dataStatus}<p class={styles.hint} role="status">{dataStatus}</p>{/if}
</section>
