<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { listen } from '@tauri-apps/api/event';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, type DonationLink } from '../api';
  import { errors } from '../errors.svelte';
  import { history } from '../history.svelte';
  import { lists } from '../lists.svelte';
  import { onAir, wheelVolume } from '../onAir.svelte';
  import { player } from '../player.svelte';
  import { share } from '../share.svelte';
  import { ui } from '../ui.svelte';
  import Icon from './Icon.svelte';
  import Waves from './Waves.svelte';
  import styles from './PlayerControls.module.css';

  const LABELS = {
    idle: 'Stopped',
    connecting: 'Connecting…',
    reconnecting: 'Reconnecting…',
    playing: 'Playing',
    paused: 'Paused',
    error: 'Error',
  } as const;

  const status = $derived(errors.player ? 'error' : player.state.status);
  // Playing, paused and stopped are already said by the play button and the title on air: the
  // line is shown only while connecting, reconnecting or failing, and stays for screen readers.
  const quiet = $derived(status === 'playing' || status === 'paused' || status === 'idle');
  const message = $derived(
    errors.player ??
      ('message' in player.state ? player.state.message : LABELS[player.state.status]),
  );
  // Station, title and artist each on their line (onAir.svelte.ts).
  const stationName = $derived(onAir.stationName);
  const station = $derived(onAir.station);
  const title = $derived(onAir.title);
  const artist = $derived(onAir.artist);
  const upNext = $derived(onAir.upNext);
  const cover = $derived(onAir.cover);

  // Volume: the filled part of the track and the speaker's waves.
  const level = $derived(player.muted ? 0 : player.volume);
  const percent = $derived(Math.round(player.volume * 100));

  // The station's donation page (docs/13-donations.md §3), asked again when the station changes
  // or a new list was downloaded. A late answer for a previous station is dropped.
  let donation = $state<DonationLink | null>(null);
  let listVersion = $state(0);
  $effect(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    listen('donations:updated', () => listVersion++)
      .then((f) => {
        if (gone) f();
        else unlisten = f;
      })
      .catch((e) => errors.report('player', e));
    return () => {
      gone = true;
      unlisten?.();
    };
  });
  $effect(() => {
    void listVersion;
    const station = player.current;
    donation = null;
    if (!station) return;
    let current = true;
    api
      .getDonationLink(station.id || null, station.url)
      .then((link) => {
        if (current) donation = link;
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  });

  // The curated lists the station is in (14 §6.4): one discreet link to the station sheet.
  $effect(() => {
    void lists.version;
    lists.refreshFor(player.current);
  });
</script>

<!-- Previous / next favorite, as in the mini player, the tray and the media keys: it changes the
     station, not the track (a live stream has no previous track). Quiet beside play. -->
{#snippet skip(forward: boolean)}
  <button
    class={styles.skip}
    type="button"
    aria-label={forward ? 'Next favorite' : 'Previous favorite'}
    title={forward ? 'Next favorite' : 'Previous favorite'}
    disabled={player.favorites.length === 0}
    onclick={() => api.switchFavorite(forward).catch((e) => errors.report('player', e))}
    ><Icon name={forward ? 'next' : 'previous'} size={18} /></button
  >
{/snippet}

<!-- Shares the station with the track on air (docs/16-sharing.md §4.1). -->
{#snippet shareButton()}
  <button
    class={styles.gear}
    type="button"
    aria-label={title ? `Share ${station} with the track on air` : `Share ${station}`}
    title="Share"
    disabled={share.busy}
    onclick={() =>
      player.current && share.offerStation(player.current, player.nowPlaying.title ?? null)}
    ><Icon name="share" size={16} /></button
  >
{/snippet}

{#snippet info()}
  <button
    class={styles.gear}
    type="button"
    aria-label="Station details"
    title="Station details"
    onclick={ui.openStation}><Icon name="info" size={16} /></button
  >
{/snippet}

<section class={styles.root} aria-label="Player">
  <div class={styles.top}>
    <!-- Always the same tile: the cover, the logo, or Soundfolk's waves when there is neither.
         The size is also set on the <img> itself, so a big cover never spills over the window. -->
    <div class={styles.art} aria-hidden="true">
      {#if cover}
        <img
          class={styles.cover}
          data-logo={player.cover.logo === true}
          src={cover}
          width="112"
          height="112"
          alt=""
          referrerpolicy="no-referrer"
          onerror={() => (onAir.brokenCover = cover)}
        />
      {:else}
        <Waves />
      {/if}
    </div>
    <div class={styles.body}>
      <!-- The station leads: the listener chose it, and it stays while tracks come and go. Each
           button ends the line it acts on: ⓘ the station's, ♡ the track's. ♡ keeps its place
           while the history logs the title, so the title never changes width under it. -->
      <div class={styles.info}>
        <div class={styles.line}>
          <p class={styles.station} title={station}>{station}</p>
          {#if player.current}{@render shareButton()}{@render info()}{/if}
        </div>
        {#if title}
          <div class={styles.line}>
            <p class={styles.title} {title}>{title}</p>
            {#if history.onAir}
              {@const track = history.onAir}
              <button
                class={styles.like}
                type="button"
                aria-pressed={track.liked}
                aria-label={track.liked ? `Unlike ${track.title}` : `Like ${track.title}`}
                title={track.liked ? 'Unlike' : 'Like'}
                data-on={track.liked}
                onclick={() => history.toggleLike({ ...track, historyId: track.id }, 'player')}
                ><Icon name="heart" size={16} filled={track.liked} /></button
              >
            {:else}
              <span class={styles.slot} aria-hidden="true"></span>
            {/if}
          </div>
        {/if}
        {#if artist}<p class={styles.artist} title={artist}>{artist}</p>{/if}
        {#if upNext}
          <p class={styles.next} title={upNext.text}>
            Up next{#if upNext.time}&nbsp;at {upNext.time}{/if}: {upNext.text}
          </p>
        {/if}
        {#if (donation || lists.forCurrent.length) && player.current}
          <p class={styles.links}>
            {#if donation}
              <button
                class={styles.donate}
                type="button"
                title={`Donate to ${stationName} (${donation.domain})`}
                onclick={() => openUrl(donation!.url).catch((e) => errors.report('player', e))}
                >Donate</button
              >
            {/if}
            {#if lists.forCurrent.length}
              <button class={styles.inLists} type="button" onclick={ui.openStation}>
                In {lists.forCurrent.length}
                {lists.forCurrent.length === 1 ? 'list' : 'lists'}
              </button>
            {/if}
          </p>
        {/if}
      </div>

      <div class={styles.row}>
        <div class={styles.transport}>
          {@render skip(false)}
          <button
            class={styles.button}
            type="button"
            data-active={player.active}
            aria-label={player.active ? 'Pause' : 'Play'}
            title={player.active ? 'Pause' : 'Play'}
            disabled={!player.current}
            onclick={() => player.toggle()}
          >
            <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              {#if player.active}
                <rect x="6" y="5" width="4" height="14" rx="1" />
                <rect x="14" y="5" width="4" height="14" rx="1" />
              {:else}
                <path
                  transform="translate(-1.5 0)"
                  d="M8 5.14v13.72a1 1 0 0 0 1.5.86l11-6.86a1 1 0 0 0 0-1.72l-11-6.86A1 1 0 0 0 8 5.14z"
                />
              {/if}
            </svg>
          </button>

          {@render skip(true)}
        </div>

        <div class={styles.volume} onwheel={wheelVolume}>
          <button
            class={styles.speaker}
            type="button"
            aria-label={player.muted ? 'Unmute' : 'Mute'}
            aria-pressed={player.muted}
            title={player.muted ? 'Unmute' : 'Mute'}
            onclick={() => player.toggleMute()}
          >
            <svg
              width="20"
              height="20"
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
    </div>
  </div>

  <p class={styles.status} data-status={status} data-quiet={quiet} role="status">
    <span class={styles.dot} aria-hidden="true"></span>
    {message}
  </p>

  {#each player.skipped as { name, reason } (name)}
    <p class={styles.skipped}>Skipped {name}: {reason}</p>
  {/each}

  {#if status === 'error' && player.goneFavorite && player.isFavorite(player.goneFavorite)}
    <p class={styles.ask}>
      Remove it from your favorites?
      <button class={styles.answer} type="button" onclick={() => player.answerGoneFavorite(true)}
        >Yes</button
      >
      <button class={styles.answer} type="button" onclick={() => player.answerGoneFavorite(false)}
        >No</button
      >
    </p>
  {/if}
</section>
