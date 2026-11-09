<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, listen, type DonationLink } from '../api';
  import { errors } from '../errors.svelte';
  import { history } from '../history.svelte';
  import { lists } from '../lists.svelte';
  import { onAir, wheelVolume } from '../onAir.svelte';
  import { player } from '../player.svelte';
  import { share } from '../share.svelte';
  import { ui } from '../ui.svelte';
  import Icon from './Icon.svelte';
  import LikeButton from './LikeButton.svelte';
  import SkipButton from './SkipButton.svelte';
  import VolumeIcon from './VolumeIcon.svelte';
  import Waves from './Waves.svelte';
  import styles from './PlayerControls.module.css';

  // Shown only while connecting, reconnecting or failing; always there for screen readers.
  const status = $derived(onAir.playerStatus);
  // Station, title and artist each on their line (onAir.svelte.ts).
  const stationName = $derived(onAir.stationName);
  const station = $derived(onAir.station);
  const title = $derived(onAir.title);
  const artist = $derived(onAir.artist);
  const upNext = $derived(onAir.upNext);
  const cover = $derived(onAir.cover);

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
        <!-- Under the station, inside the block: connecting never changes its height. -->
        <p
          class={styles.status}
          data-status={status.status}
          data-quiet={status.quiet}
          role="status"
        >
          <span class={styles.dot} aria-hidden="true"></span>
          {status.text}
        </p>
        {#if title}
          <div class={styles.line}>
            <p class={styles.title} {title}>{title}</p>
            {#if history.onAir}
              <LikeButton class={styles.like} />
            {:else}
              <span class={styles.slot} aria-hidden="true"></span>
            {/if}
          </div>
        {/if}
        {#if artist}<p class={styles.artist} title={artist}>{artist}</p>{/if}
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
        {#if upNext}
          <p class={styles.next} title={upNext.text}>
            Up next{#if upNext.time}&nbsp;at {upNext.time}{/if}: {upNext.text}
          </p>
        {/if}
      </div>

      <div class={styles.row}>
        <div class={styles.transport}>
          <!-- Quiet beside play. -->
          <SkipButton forward={false} class={styles.skip} size={18} />
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

          <SkipButton forward class={styles.skip} size={18} />
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
            <VolumeIcon size={20} />
          </button>
          <input
            class={styles.slider}
            type="range"
            min="0"
            max="1"
            step="0.01"
            aria-label="Volume"
            aria-valuetext={onAir.volumeText}
            title={`Volume ${onAir.volumePercent} %`}
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

  {#each player.skipped as { name, reason } (name)}
    <p class={styles.skipped}>Skipped {name}: {reason}</p>
  {/each}

  {#if status.status === 'error' && player.goneFavorite && player.isFavorite(player.goneFavorite)}
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
