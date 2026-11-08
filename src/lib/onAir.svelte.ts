// SPDX-License-Identifier: MPL-2.0
// What the player shows of the station on air, shared by the player and the mini player so that
// both name the track and pick the cover the same way.
import { untrack } from 'svelte';
import { history } from './history.svelte';
import { displayTitle } from './trackLinks';
import { player } from './player.svelte';
import { wheelSteps } from './wheel';
import { stationNames } from './stationNames.svelte';

class OnAir {
  // A new station shows its name at once; what was on air before (title, artist, up next, cover)
  // is hidden until the backend sends something for the new one, so the new station never seems
  // to play the old track. The backend clears the title as soon as playback changes, and
  // announces the new favorite before playing it: the next event is always the new station's.
  // So each is kept with the station it was received for, and shown only while that station is
  // on air.
  private titleFor = $state<string | null>(null);
  private coverFor = $state<string | null>(null);
  private staleTitle = $derived((player.current?.id ?? null) !== this.titleFor);
  private staleCover = $derived((player.current?.id ?? null) !== this.coverFor);

  constructor() {
    $effect.root(() => {
      $effect(() => {
        void player.nowPlaying;
        untrack(() => (this.titleFor = player.current?.id ?? null));
      });
      $effect(() => {
        void player.cover;
        untrack(() => (this.coverFor = player.current?.id ?? null));
      });
    });
  }

  private nowPlaying = $derived(
    this.staleTitle ? { station: null, title: null } : player.nowPlaying,
  );

  /** The station as named in the list: the stream's own `icy-name` is often a mount point
   *  ("fipworld-hifi.aac") or a placeholder. A name the user gave it comes first. */
  stationName = $derived(
    player.current ? stationNames.shown(player.current.id, player.current.name) : null,
  );
  /** The first line of the player: the station, which the listener chose and which does not
   *  change from one track to the next. */
  station = $derived(this.stationName ?? 'No station');
  // Artist and title come apart from the source when it gives them that way, otherwise as the
  // history split them; a title the history did not log (a jingle) is shown whole. A title that
  // only repeats the station's name is no track.
  private split = $derived(this.nowPlaying.track ?? history.onAir);
  private rawTitle = $derived(this.split?.title ?? this.nowPlaying.title ?? null);
  private repeatsStation = $derived(
    !!this.rawTitle &&
      !!this.stationName &&
      this.rawTitle.trim().toLowerCase() === this.stationName.trim().toLowerCase(),
  );
  /** The track on air, under the station; null without one. */
  title = $derived(this.nowPlaying.title && !this.repeatsStation ? this.rawTitle : null);
  artist = $derived(this.title ? (this.split?.artist ?? null) : null);
  /** Title and artist on one line, where there is room for one only. */
  track = $derived(this.title && this.artist ? `${this.title} – ${this.artist}` : this.title);

  /** Announced by the station's page: the next track, and when it starts. */
  upNext = $derived.by(() => {
    const next = this.nowPlaying.next;
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
    if (this.staleCover || !url || title !== this.nowPlaying.title || url === this.brokenCover)
      return null;
    return /^(https?:\/\/|data:image\/)/i.test(url) ? url : null;
  });
  /** The cover is the station's logo, not the track's art. */
  isLogo = $derived(player.cover.logo === true);
}

export const onAir = new OnAir();

let wheelCarry = 0;
let saveTimer: ReturnType<typeof setTimeout> | undefined;

/** The wheel over the volume (`wheelSteps`): 1 % per notch, saved once the wheel rests. */
export function wheelVolume(e: WheelEvent) {
  if (!(e.deltaY || e.deltaX)) return;
  e.preventDefault();
  const { steps, carry } = wheelSteps(wheelCarry, e);
  wheelCarry = carry;
  if (!steps) return;
  player.setVolume(Math.round(player.volume * 100 - steps) / 100);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => player.saveVolume(), 400);
}
