<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { history } from '../history.svelte';
  import { player } from '../player.svelte';
  import { ui } from '../ui.svelte';
  import styles from './PlayerControls.module.css';

  const LABELS = {
    idle: 'Stopped',
    connecting: 'Connecting…',
    reconnecting: 'Reconnecting…',
    playing: 'Playing',
    paused: 'Paused',
    error: 'Error',
  } as const;

  const status = $derived(player.error ? 'error' : player.state.status);
  const message = $derived(
    player.error ??
      ('message' in player.state ? player.state.message : LABELS[player.state.status]),
  );
  // Current title if the stream announces one, otherwise the selected station.
  // The station is named as in the list: the stream's own `icy-name` is often
  // a mount point ("fipworld-hifi.aac") or a placeholder.
  const headline = $derived(player.nowPlaying.title ?? player.current?.name ?? 'No station');
  const subline = $derived(player.nowPlaying.title ? player.current?.name : null);
  // Announced by the station's page: the next track, and when it starts.
  const upNext = $derived.by(() => {
    const next = player.nowPlaying.next;
    if (!next) return null;
    const text = next.artist ? `${next.artist} – ${next.title}` : next.title;
    const at = next.started_at ? new Date(next.started_at) : null;
    const time = at && !Number.isNaN(at.getTime()) ? at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : null;
    return { text, time };
  });

  // Cover art of the track on air, from the sources set in ⚙ › Cover art (only while it is
  // still the title it was found for), or the station's logo. An image that fails to load is
  // hidden rather than shown broken.
  let brokenCover = $state<string | null>(null);
  const cover = $derived.by(() => {
    const { title, url } = player.cover;
    if (!url || title !== player.nowPlaying.title || url === brokenCover) return null;
    return /^https?:\/\//i.test(url) ? url : null;
  });

  // Volume: the filled part of the track, the speaker's waves and a wheel step of 5 %.
  const level = $derived(player.muted ? 0 : player.volume);
  const percent = $derived(Math.round(player.volume * 100));
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  function onWheel(e: WheelEvent) {
    const delta = e.deltaY || -e.deltaX;
    if (!delta) return;
    e.preventDefault();
    player.setVolume(Math.round((player.volume + (delta < 0 ? 0.05 : -0.05)) * 100) / 100);
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => player.saveVolume(), 400);
  }

  // Back from the settings: focus returns to the gear that opened them.
  let gear: HTMLButtonElement | undefined = $state();
  let wasOpen = false;
  $effect(() => {
    const open = ui.settingsOpen;
    if (wasOpen && !open) gear?.focus();
    wasOpen = open;
  });
</script>

<section class={styles.root} aria-label="Player">
  <div class={styles.top}>
    <!-- Always the same tile: the cover, the logo, or a plain note when there is neither.
         The size is also set on the <img> itself, so a big cover never spills over the window. -->
    <div class={styles.art} aria-hidden="true">
      {#if cover}
        <img
          class={styles.cover}
          data-logo={player.cover.logo === true}
          src={cover}
          width="56"
          height="56"
          alt=""
          referrerpolicy="no-referrer"
          onerror={() => (brokenCover = cover)}
        />
      {:else}
        <svg class={styles.placeholder} width="28" height="28" viewBox="0 0 24 24" fill="currentColor">
          <path d="M9 18.5a2.5 2.5 0 1 1-2-2.45V5.2l12-2.2v12.5a2.5 2.5 0 1 1-2-2.45V7.3l-8 1.47z" />
        </svg>
      {/if}
    </div>
    <div class={styles.info}>
      <p class={styles.headline} title={headline}>{headline}</p>
      {#if subline}<p class={styles.subline}>{subline}</p>{/if}
      {#if upNext}
        <p class={styles.next} title={upNext.text}>
          Up next{#if upNext.time}&nbsp;at {upNext.time}{/if}: {upNext.text}
        </p>
      {/if}
    </div>
    {#if history.onAir}
      {@const track = history.onAir}
      <button
        class={styles.like}
        type="button"
        aria-pressed={track.liked}
        aria-label={track.liked ? `Unlike ${track.title}` : `Like ${track.title}`}
        data-on={track.liked}
        onclick={() => history.toggleLike({ ...track, historyId: track.id })}>{track.liked ? '♥' : '♡'}</button
      >
    {/if}
    <button
      class={styles.gear}
      type="button"
      aria-label="Station details"
      title="Station details"
      disabled={!player.current}
      onclick={ui.openStation}>ⓘ</button
    >
    <button
      class={styles.gear}
      type="button"
      aria-label="Settings"
      title="Settings"
      bind:this={gear}
      onclick={ui.openSettings}>⚙&#xFE0E;</button
    >
  </div>

  <div class={styles.row}>
    <button
      class={styles.button}
      type="button"
      data-active={player.active}
      disabled={!player.current}
      onclick={() => player.toggle()}
    >
      {player.active ? 'Stop' : 'Play'}
    </button>

    <div class={styles.volume} onwheel={onWheel}>
      <button
        class={styles.speaker}
        type="button"
        aria-label={player.muted ? 'Unmute' : 'Mute'}
        aria-pressed={player.muted}
        title={player.muted ? 'Unmute' : 'Mute'}
        onclick={() => player.toggleMute()}
      >
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M11 5 6 9H3v6h3l5 4z" fill="currentColor" />
          {#if level === 0}
            <path d="m16 9 5 6m0-6-5 6" />
          {:else}
            <path d="M15.5 9.5a3.5 3.5 0 0 1 0 5" />
            {#if level > 0.5}<path d="M18.5 6.5a7.5 7.5 0 0 1 0 11" />{/if}
          {/if}
        </svg>
      </button>
      <input
        class={styles.slider}
        type="range"
        min="0"
        max="1"
        step="0.01"
        aria-label="Volume"
        aria-valuetext={player.muted ? `Muted (${percent} %)` : `${percent} %`}
        title={`Volume ${percent} %`}
        data-muted={player.muted}
        style:--fill={`${player.volume * 100}%`}
        value={player.volume}
        oninput={(e) => player.setVolume(e.currentTarget.valueAsNumber)}
        onchange={() => player.saveVolume()}
      />
    </div>
  </div>

  <p class={styles.status} data-status={status} role="status">
    <span class={styles.dot} aria-hidden="true"></span>
    {message}
  </p>

  {#if status === 'error' && player.goneFavorite && player.isFavorite(player.goneFavorite)}
    <p class={styles.ask}>
      Remove it from your favorites?
      <button class={styles.answer} type="button" onclick={() => player.answerGoneFavorite(true)}>Yes</button>
      <button class={styles.answer} type="button" onclick={() => player.answerGoneFavorite(false)}>No</button>
    </p>
  {/if}
</section>
