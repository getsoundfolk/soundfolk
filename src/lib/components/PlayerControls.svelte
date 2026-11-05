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
    {#if cover}
      <img
        class={styles.cover}
        data-logo={player.cover.logo === true}
        src={cover}
        alt=""
        referrerpolicy="no-referrer"
        onerror={() => (brokenCover = cover)}
      />
    {/if}
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

    <input
      class={styles.volume}
      type="range"
      min="0"
      max="1"
      step="0.01"
      aria-label="Volume"
      value={player.volume}
      oninput={(e) => player.setVolume(e.currentTarget.valueAsNumber)}
      onchange={() => player.saveVolume()}
    />
  </div>

  <p class={styles.status} data-status={status} role="status">
    <span class={styles.dot} aria-hidden="true"></span>
    {message}
  </p>
</section>
