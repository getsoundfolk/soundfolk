<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- The whole mini player window (src-tauri/src/mini.rs). The same front end as the main window
     (`main.ts` mounts this one when the window is the mini player's), so it reads the same stores
     and calls the same commands.

     The window has no title bar and is transparent: this component is the window. It draws the
     rounded card, and every part of it moves the window except the controls
     (`data-tauri-drag-region="deep"`; Tauri leaves buttons and inputs out by itself).

     Two layouts, full (560 × 180) and compact (320 × 88). The window's width says which: the
     backend sizes the window and keeps the choice, so the mode has one copy only. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Favorite, type HistoryEntry, type Playable } from '../api';
  import { errors } from '../errors.svelte';
  import { history } from '../history.svelte';
  import { onAir, wheelVolume } from '../onAir.svelte';
  import { player } from '../player.svelte';
  import { stationNames } from '../stationNames.svelte';
  import { shortcutKeydown } from '../shortcuts';
  import { displayTitle } from '../trackLinks';
  import Icon from './Icon.svelte';
  import Waves from './Waves.svelte';
  import styles from './MiniPlayer.module.css';

  onMount(() => {
    const ready = player.init();
    stationNames.init();
    return () => void ready.then((off) => off());
  });

  // The backend logs a title before announcing it: the ♥ then finds it in the history. This
  // window shows only the player's errors.
  $effect(() => {
    void player.nowPlaying.title;
    history.refreshLatest('player');
  });

  // Zoom never applies to this window: CSS pixels are the logical size set by the backend.
  let width = $state(window.innerWidth);
  const compact = $derived(width < 440);
  // A resize that failed leaves the window, so the layout, as it was.
  const setCompact = (on: boolean) => api.setMiniCompact(on).catch(() => {});
  // The command takes this very window down: its answer lands nowhere.
  const back = () => api.closeMiniPlayer().catch(() => {});

  const cover = $derived(onAir.cover);
  const status = $derived(errors.player ? 'error' : player.state.status);
  // Said only while it matters, as in the player: connecting, reconnecting, an error.
  const message = $derived(
    errors.player ??
      (status === 'connecting'
        ? 'Connecting…'
        : status === 'reconnecting'
          ? 'Reconnecting…'
          : status === 'error' && 'message' in player.state
            ? player.state.message
            : null),
  );
  const track = $derived(history.onAir);
  const level = $derived(player.muted ? 0 : player.volume);
  const percent = $derived(Math.round(player.volume * 100));
  const noFavorites = $derived(player.favorites.length === 0);

  // The right column: the favorites (a click plays one) or the titles heard lately. Not kept:
  // the favorites are the useful default.
  let tab = $state<'favorites' | 'recent'>('favorites');
  let recent = $state<HistoryEntry[]>([]);
  $effect(() => {
    if (tab !== 'recent') return;
    void player.nowPlaying.title;
    void history.version;
    let current = true;
    api
      .getListeningHistory(12)
      .then((entries) => {
        // The same track twice in a row (heard again after a stop) is one line here.
        if (current) recent = entries.filter((e, i) => e.track_key !== entries[i - 1]?.track_key);
      })
      .catch((e) => errors.report('player', e));
    return () => {
      current = false;
    };
  });

  const fromFavorite = (f: Favorite): Playable => ({
    id: f.station_id,
    name: f.name,
    url: f.stream_url,
    genre: f.genre,
  });

  // The station on air stays in sight in the list.
  let list: HTMLElement | undefined = $state();
  $effect(() => {
    void player.current?.id;
    void tab;
    void compact;
    list?.querySelector('[aria-current="true"]')?.scrollIntoView({ block: 'nearest' });
  });

  // The volume slider shows while the pointer is over the speaker, and for as long as it is
  // dragged: a pointer leaving the thin strip mid-drag must not fold it under its own thumb.
  let volumeHover = $state(false);
  let volumeDrag = $state(false);
  const volumeOpen = $derived(volumeHover || volumeDrag);

  // A double click on a drag region maximizes the window (Tauri's drag script): a mini player
  // filling the screen is never wanted. Caught first, on the way down.
  onMount(() => {
    const stop = (e: MouseEvent) => {
      if (e.detail > 1) e.stopPropagation();
    };
    window.addEventListener('mousedown', stop, { capture: true });
    return () => window.removeEventListener('mousedown', stop, { capture: true });
  });

  const switchFavorite = (forward: boolean) =>
    api.switchFavorite(forward).catch((e) => errors.report('player', e));
</script>

<!-- On the window, not the slider: a drag that ends elsewhere never sends pointerup to it. -->
<!-- Space and M as in the main window (the search shortcuts find nothing to focus here). -->
<svelte:window
  bind:innerWidth={width}
  onpointerup={() => (volumeDrag = false)}
  onkeydown={shortcutKeydown}
/>

{#snippet playButton()}
  <button
    class={styles.play}
    type="button"
    data-active={player.active}
    aria-label={player.active ? 'Pause' : 'Play'}
    title={player.active ? 'Pause' : 'Play'}
    disabled={!player.current}
    onclick={() => player.toggle()}
    ><Icon name={player.active ? 'pause' : 'play'} size={compact ? 14 : 16} /></button
  >
{/snippet}

{#snippet skip(forward: boolean)}
  <button
    class={styles.control}
    type="button"
    aria-label={forward ? 'Next favorite' : 'Previous favorite'}
    title={forward ? 'Next favorite' : 'Previous favorite'}
    disabled={noFavorites}
    onclick={() => switchFavorite(forward)}><Icon name={forward ? 'next' : 'previous'} /></button
  >
{/snippet}

{#snippet like(onArt: boolean)}
  {#if track}
    {@const t = track}
    <button
      class={onArt ? styles.artButton : styles.control}
      type="button"
      aria-pressed={t.liked}
      aria-label={t.liked ? `Unlike ${t.title}` : `Like ${t.title}`}
      title={t.liked ? 'Unlike' : 'Like'}
      data-like={t.liked}
      onclick={() => history.toggleLike({ ...t, historyId: t.id }, 'player')}
      ><Icon name="heart" filled={t.liked} /></button
    >
  {/if}
{/snippet}

{#snippet art(className: string)}
  <div class={className} aria-hidden="true">
    {#key cover}
      {#if cover}
        <img
          src={cover}
          alt=""
          data-logo={onAir.isLogo}
          referrerpolicy="no-referrer"
          onerror={() => (onAir.brokenCover = cover)}
        />
      {:else}
        <Waves />
      {/if}
    {/key}
  </div>
{/snippet}

<div class={styles.root} data-compact={compact} data-tauri-drag-region="deep">
  {#if compact}
    {@render art(styles.compactArt)}
    <button
      class={styles.corner}
      type="button"
      aria-label="Enlarge the mini player"
      title="Enlarge the mini player"
      onclick={() => setCompact(false)}><Icon name="expand" size={14} /></button
    >
    <div class={styles.compactBody}>
      <p class={styles.compactTitle} title={onAir.station}>{onAir.station}</p>
      {#if message}
        <p class={styles.compactLine} data-error={status === 'error'} title={message}>
          {message}
        </p>
      {:else if onAir.track}
        <p class={styles.compactLine} title={onAir.track}>{onAir.track}</p>
      {/if}
      <div class={styles.compactRow}>
        {@render skip(false)}
        {@render playButton()}
        {@render skip(true)}
        <span class={styles.push}></span>
        {@render like(false)}
      </div>
    </div>
  {:else}
    {@render art(styles.art)}
    <div class={styles.shade} aria-hidden="true"></div>

    <!-- The way back to the full player: not part of the design, so shown only on hover. -->
    <button
      class={styles.back}
      type="button"
      aria-label="Back to the full player"
      title="Back to the full player"
      onclick={back}><Icon name="window" size={14} /></button
    >

    <section class={styles.left} aria-label="Player">
      <div class={styles.tools}>
        <!-- The slider grows leftwards out of the speaker, into the empty part of the row: the
             buttons on its right never move. -->
        <div
          class={styles.volume}
          role="group"
          aria-label="Volume"
          onpointerenter={() => (volumeHover = true)}
          onpointerleave={() => (volumeHover = false)}
          onwheel={wheelVolume}
        >
          <input
            class={styles.slider}
            type="range"
            min="0"
            max="1"
            step="0.01"
            data-open={volumeOpen}
            data-muted={player.muted}
            aria-label="Volume"
            aria-valuetext={player.muted ? `Muted (${percent} %)` : `${percent} %`}
            title={`Volume ${percent} %`}
            style:--fill={`${player.volume * 100}%`}
            value={player.volume}
            onpointerdown={() => (volumeDrag = true)}
            oninput={(e) => player.setVolume(e.currentTarget.valueAsNumber)}
            onchange={() => player.saveVolume()}
          />
          <button
            class={styles.artButton}
            type="button"
            aria-label={player.muted ? 'Unmute' : 'Mute'}
            aria-pressed={player.muted}
            title={player.muted ? 'Unmute' : 'Mute'}
            onclick={() => player.toggleMute()}
          >
            <svg
              width="16"
              height="16"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="M11 5 6 9H3v6h3l5 4z" fill="currentColor" />
              {#if level === 0}
                <path d="m16 9 5 6m0-6-5 6" />
              {:else}
                <path d="M15.5 9.5a3.5 3.5 0 0 1 0 5" />
                {#if level > 0.5}<path d="M18.5 6.5a7.5 7.5 0 0 1 0 11" />{/if}
              {/if}
            </svg>
          </button>
        </div>
        <button
          class={styles.artButton}
          type="button"
          aria-label="Shrink the mini player"
          title="Shrink the mini player"
          onclick={() => setCompact(true)}><Icon name="shrink" /></button
        >
        {@render like(true)}
      </div>

      <div class={styles.text}>
        <p class={styles.station} title={onAir.station}>{onAir.station}</p>
        {#if onAir.title}<p class={styles.title} title={onAir.title}>{onAir.title}</p>{/if}
        {#if onAir.artist}<p class={styles.artist} title={onAir.artist}>{onAir.artist}</p>{/if}
        {#if message}
          <p class={styles.message} data-error={status === 'error'} role="status" title={message}>
            {message}
          </p>
        {:else if onAir.upNext}
          {@const next = onAir.upNext}
          <p class={styles.message} title={next.text}>
            Up next{#if next.time}&nbsp;at {next.time}{/if}: {next.text}
          </p>
        {/if}
      </div>
    </section>

    <section class={styles.right} aria-label={tab === 'favorites' ? 'Favorites' : 'Recently heard'}>
      <div class={styles.list} bind:this={list}>
        {#if tab === 'favorites'}
          {#each player.favorites as f (f.station_id)}
            {@const name = stationNames.shown(f.station_id, f.name)}
            {@const on = f.station_id === player.current?.id}
            <button
              class={styles.item}
              type="button"
              aria-current={on}
              title={name}
              onclick={() => player.play(fromFavorite(f))}>{name}</button
            >
          {:else}
            <p class={styles.empty}>No favorites yet: star a station in the full player.</p>
          {/each}
        {:else}
          {#each recent as entry (entry.id)}
            {@const text = displayTitle(entry)}
            <p class={styles.entry} title={text}>
              <span class={styles.entryTitle}>{entry.title}</span>
              {#if entry.artist}<span class={styles.entryArtist}>{entry.artist}</span>{/if}
            </p>
          {:else}
            <p class={styles.empty}>Nothing heard yet.</p>
          {/each}
        {/if}
      </div>

      <div class={styles.transport}>
        {@render skip(false)}
        {@render playButton()}
        {@render skip(true)}
        <button
          class={`${styles.control} ${styles.tab}`}
          type="button"
          aria-pressed={tab === 'recent'}
          aria-label={tab === 'recent' ? 'Show the favorites' : 'Show the titles heard lately'}
          title={tab === 'recent' ? 'Favorites' : 'Recently heard'}
          onclick={() => (tab = tab === 'recent' ? 'favorites' : 'recent')}
          ><Icon name="history" /></button
        >
      </div>
    </section>
  {/if}
</div>
