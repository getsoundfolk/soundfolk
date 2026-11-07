// SPDX-License-Identifier: MPL-2.0
// What the player shows of the station on air, shared by the player and the mini player so that
// both name the track and pick the cover the same way.
import { history } from './history.svelte';
import { displayTitle } from './trackLinks';
import { player } from './player.svelte';
import { stationNames } from './stationNames.svelte';

class OnAir {
  /** The station as named in the list: the stream's own `icy-name` is often a mount point
   *  ("fipworld-hifi.aac") or a placeholder. A name the user gave it comes first. */
  stationName = $derived(
    player.current ? stationNames.shown(player.current.id, player.current.name) : null,
  );
  // Artist and title come apart from the source when it gives them that way, otherwise as the
  // history split them; a title the history did not log (the station's own name, a jingle) is
  // shown whole. Without a title, the station alone.
  private split = $derived(player.nowPlaying.track ?? history.onAir);
  headline = $derived(
    this.split?.title ?? player.nowPlaying.title ?? this.stationName ?? 'No station',
  );
  artist = $derived(player.nowPlaying.title ? (this.split?.artist ?? null) : null);
  /** The station, under a title; null when the headline already is the station. */
  subline = $derived(player.nowPlaying.title ? this.stationName : null);

  /** Announced by the station's page: the next track, and when it starts. */
  upNext = $derived.by(() => {
    const next = player.nowPlaying.next;
    if (!next) return null;
    const text = displayTitle(next);
    const at = next.started_at ? new Date(next.started_at) : null;
    const time =
      at && !Number.isNaN(at.getTime())
        ? at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
        : null;
    return { text, time };
  });

  /** An image that failed to load: hidden rather than shown broken. */
  brokenCover = $state<string | null>(null);
  /** Cover art of the track on air (only while it is still the title it was found for), or the
   *  station's logo (inline, kept on disk by the backend). */
  cover = $derived.by(() => {
    const { title, url } = player.cover;
    if (!url || title !== player.nowPlaying.title || url === this.brokenCover) return null;
    return /^(https?:\/\/|data:image\/)/i.test(url) ? url : null;
  });
  /** The cover is the station's logo, not the track's art. */
  isLogo = $derived(player.cover.logo === true);
}

export const onAir = new OnAir();

// The wheel is the fine control of the volume: 1 % per mouse notch, where a click on the track
// jumps. A touchpad sends many small pixel deltas: they add up, one step per notch's worth.
const NOTCH_PX = 100;
let wheelPx = 0;
let saveTimer: ReturnType<typeof setTimeout> | undefined;

export function wheelVolume(e: WheelEvent) {
  const raw = e.deltaY || -e.deltaX;
  if (!raw) return;
  e.preventDefault();
  const px =
    e.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? raw * 33
      : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
        ? raw * NOTCH_PX * 3
        : raw;
  // A direction change drops what was left over from the other way.
  if (Math.sign(px) !== Math.sign(wheelPx)) wheelPx = 0;
  wheelPx += px;
  // At least one step per event: a mouse notch must never do nothing.
  const steps =
    Math.trunc(wheelPx / NOTCH_PX) || (Math.abs(px) >= NOTCH_PX / 2 ? Math.sign(px) : 0);
  if (!steps) return;
  wheelPx = Math.abs(wheelPx) >= NOTCH_PX ? wheelPx - steps * NOTCH_PX : 0;
  player.setVolume(Math.round(player.volume * 100 - steps) / 100);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => player.saveVolume(), 400);
}
