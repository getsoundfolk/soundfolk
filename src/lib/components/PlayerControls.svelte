<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { history } from '../history.svelte';
  import { player } from '../player.svelte';
  import SettingsMenu from './SettingsMenu.svelte';
  import styles from './PlayerControls.module.css';

  const LABELS = {
    idle: 'Stopped',
    connecting: 'Connecting…',
    playing: 'Playing',
    paused: 'Paused',
    error: 'Error',
  } as const;

  const status = $derived(player.error ? 'error' : player.state.status);
  const message = $derived(
    player.error ??
      (player.state.status === 'error' ? player.state.message : LABELS[player.state.status]),
  );
  // Current title if the stream announces one, otherwise the selected station.
  const headline = $derived(player.nowPlaying.title ?? player.current?.name ?? 'No station');
  const subline = $derived(
    player.nowPlaying.title ? (player.nowPlaying.station ?? player.current?.name) : null,
  );
</script>

<section class={styles.root} aria-label="Player">
  <div class={styles.top}>
    <div class={styles.info}>
      <p class={styles.headline} title={headline}>{headline}</p>
      {#if subline}<p class={styles.subline}>{subline}</p>{/if}
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
    <SettingsMenu />
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
    />
  </div>

  <p class={styles.status} data-status={status} role="status">
    <span class={styles.dot} aria-hidden="true"></span>
    {message}
  </p>
</section>
