<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- A received link that is not a catalog station nor a known list (docs/16-sharing.md §3.3): a
     station added by hand by the sender, a listener's list, a station or list gone, or a broken
     link. Nothing plays or is saved until the listener asks; a station from a link stays off the
     local network (§6.2). -->
<script lang="ts">
  import { api, errorMessage, type PersonalStation, type Station } from '../api';
  import { player } from '../player.svelte';
  import { share } from '../share.svelte';
  import { hostOf } from '../stations';
  import { ui } from '../ui.svelte';
  import Icon from './Icon.svelte';
  import styles from './StationSheet.module.css';

  let dialog: HTMLDialogElement | undefined = $state();
  const received = $derived(share.received);
  const link = $derived(received && 'link' in received ? received.link : null);

  /** Per stream address: being added, added, or why it failed. */
  let adding = $state<Record<string, 'adding' | PersonalStation | { error: string }>>({});
  /** The catalog station with the received personal station's stream, if any. */
  let inCatalog = $state<Station | null>(null);
  /** A received list's catalog stations, in its order (the ones gone are left out). */
  let listStations = $state<Station[] | null>(null);
  let listError = $state<string | null>(null);

  $effect(() => {
    if (!dialog) return;
    if (received && !dialog.open) dialog.showModal();
    else if (!received && dialog.open) dialog.close();
  });

  // What the link names, read from the catalog once the sheet shows it.
  $effect(() => {
    adding = {};
    inCatalog = null;
    listStations = null;
    listError = null;
    const shown = received;
    if (shown?.kind === 'personal_station') {
      api
        .catalogStationForStream(shown.link.url)
        .then((found) => {
          if (share.received === shown) inCatalog = found;
        })
        .catch(() => {});
    } else if (shown?.kind === 'personal_list') {
      // `null`: gone from the catalog. A failed read is not that: it is shown as an error.
      Promise.all(shown.link.stations.map((id) => api.getStation(id)))
        .then((found) => {
          if (share.received === shown)
            listStations = found.filter((s): s is Station => s !== null);
        })
        .catch((e) => {
          if (share.received === shown) listError = errorMessage(e);
        });
    }
  });

  const title = $derived.by(() => {
    switch (received?.kind) {
      case 'personal_station':
        return received.link.name;
      case 'personal_list':
        return received.link.title;
      case 'gone':
        return 'Station not found';
      case 'unknown_list':
        return 'List not found';
      default:
        return 'Link';
    }
  });

  async function add(name: string, url: string) {
    adding[url] = 'adding';
    try {
      adding[url] = await api.addPersonalStation(url, name, true);
      ui.personalAdded();
    } catch (e) {
      adding[url] = { error: errorMessage(e) };
    }
  }

  function showCatalog(station: Station) {
    share.closeReceived();
    ui.showStation(station);
  }
</script>

{#snippet addButton(name: string, url: string)}
  {@const state = adding[url]}
  {#if state === 'adding'}
    <button class={styles.button} type="button" disabled aria-busy="true"
      >Testing the stream…</button
    >
  {:else if state && 'station_id' in state}
    <button
      class={styles.primary}
      type="button"
      onclick={() => player.play({ id: state.station_id, name: state.name, url: state.stream_url })}
      ><Icon name="play" size={14} /> Play</button
    >
    <span class={styles.saved}>Added to My stations</span>
  {:else}
    <button class={styles.button} type="button" onclick={() => add(name, url)}
      >Add to My stations</button
    >
  {/if}
  {#if state && typeof state === 'object' && 'error' in state}
    <p class={styles.warning} role="alert">{state.error}</p>
  {/if}
{/snippet}

<dialog
  class={styles.sheet}
  bind:this={dialog}
  aria-labelledby="shared-sheet-title"
  onclose={share.closeReceived}
  onclick={(e) => e.target === dialog && share.closeReceived()}
>
  {#if received}
    <header class={styles.header}>
      <h2 class={styles.title} id="shared-sheet-title">{title}</h2>
      <button class={styles.close} type="button" aria-label="Close" onclick={share.closeReceived}
        ><Icon name="x" /></button
      >
    </header>

    <div class={styles.section}>
      {#if link?.from}<p class={styles.text}>From {link.from}</p>{/if}
      {#if link?.track}
        <p class={styles.text}>
          {link.from ? `${link.from} was listening to` : 'On air when the link was made:'}
          {link.track}
        </p>
      {/if}

      {#if received.kind === 'invalid'}
        <p class={styles.warning} role="alert">{received.message}</p>
      {:else if received.kind === 'gone'}
        <p class={styles.text}>This station is no longer in the catalog.</p>
        <p class={`${styles.hint} ${styles.mono}`}>{received.link.id}</p>
      {:else if received.kind === 'unknown_list'}
        <p class={styles.text}>
          This list is not published: it was removed, or it is not merged yet.
        </p>
        <p class={`${styles.hint} ${styles.mono}`}>{received.link.id}</p>
      {:else if received.kind === 'personal_station'}
        <p class={styles.text}>
          Added by hand by whoever sent the link: it is not in the catalog under this address.
        </p>
        <dl class={styles.details}>
          <div>
            <dt>Stream from</dt>
            <dd class={styles.mono}>{hostOf(received.link.url)}</dd>
          </div>
        </dl>
        {#if inCatalog}
          {@const station = inCatalog}
          <p class={styles.text}>The catalog has this stream as {station.name}.</p>
          <div class={styles.actions}>
            <button class={styles.primary} type="button" onclick={() => showCatalog(station)}
              >Open {station.name}</button
            >
          </div>
        {/if}
        <div class={styles.actions}>
          {@render addButton(received.link.name, received.link.url)}
        </div>
        <p class={styles.hint}>
          The stream is tested before it is added. A station from a link cannot reach your local
          network.
        </p>
      {:else if received.kind === 'personal_list'}
        {@const count = received.link.stations.length + received.link.personal.length}
        <p class={styles.text}>
          A list of {count}
          {count === 1 ? 'station' : 'stations'}. Saving a list as yours comes later: open its
          stations one by one.
        </p>
        {#if listError}<p class={styles.warning} role="alert">{listError}</p>{/if}
        {#if listStations === null && received.link.stations.length && !listError}
          <p class={styles.hint}>Reading the stations from the catalog…</p>
        {:else if listStations}
          <div class={styles.actions}>
            {#each listStations as station (station.id)}
              <button class={styles.chip} type="button" onclick={() => showCatalog(station)}
                >{station.name}</button
              >
            {/each}
          </div>
          {#if listStations.length < received.link.stations.length}
            <p class={styles.hint}>
              {received.link.stations.length - listStations.length} of its stations are no longer in the
              catalog.
            </p>
          {/if}
        {/if}
        {#each received.link.personal as entry, i (i)}
          <div class={styles.result}>
            <p>{entry.name} <span class={styles.hint}>· {hostOf(entry.url)}</span></p>
            <div class={styles.actions}>{@render addButton(entry.name, entry.url)}</div>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</dialog>
