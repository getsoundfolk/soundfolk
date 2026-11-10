// SPDX-License-Identifier: MPL-2.0
// Shared player state (runes): a single subscription to the backend events.
import {
  api,
  listen,
  type Cover,
  type Favorite,
  type NowPlaying,
  type Playable,
  type PlayerState,
  type RecipeNotice,
  type Skipped,
  type TitleStatus,
} from './api';
import { errors } from './errors.svelte';
import { playableOf } from './stations';

/** How long a surprise station has to be heard before the next one is tried. Working streams
 *  were measured at 0.6 to 1 s to the first sound (SomaFM near 1 s): twice the slowest. */
const SURPRISE_WAIT_MS = 2000;
/** `run` leaves the player's error in place. */
const KEEP_ERROR = true;
/** Catalog draws one Surprise me may make before it gives up. */
const SURPRISE_DRAWS = 3;

/** No station, no title. */
export const NOTHING_ON_AIR: NowPlaying = {
  station: null,
  title: null,
  track: null,
  next: null,
  show: null,
  source: null,
};

class PlayerStore {
  state = $state<PlayerState>({ status: 'idle' });
  nowPlaying = $state<NowPlaying>(NOTHING_ON_AIR);
  /** Cover art of the track on air, chosen by the backend from the sources set in ⚙. */
  cover = $state<Cover>({ title: null, url: null, logo: false });
  current = $state<Playable | null>(null);
  volume = $state(0.8);
  /** Sound cut by the speaker button, M or the tray; the slider keeps its level to come back to.
   *  Held by the backend, so that every window shows the same. Not saved. */
  muted = $state(false);
  favorites = $state<Favorite[]>([]);
  clickNotice = $state(false);
  /** Where the title of the current playback comes from (station sheet). */
  titleStatus = $state<TitleStatus | null>(null);
  /** What a local recipe the user just saved reads (or why it is not used). */
  recipeNotice = $state<RecipeNotice | null>(null);
  /** Favorite that failed and is no longer in the catalog: the player offers to remove it. */
  goneFavorite = $state<string | null>(null);
  /** Favorites that Next / Previous skipped to reach the station on air. */
  skipped = $state<Skipped[]>([]);

  /** Surprise me is looking for a station that plays. */
  surprising = $state(false);
  /** Stations already offered by Surprise me in this run: never twice. */
  private surprised = new Set<string>();

  /** Audio is (or is about to be) playing: the main button shows Pause. */
  active = $derived(['connecting', 'reconnecting', 'playing'].includes(this.state.status));

  /** Closes the recipe notice; with `notice`, only if it is still that one (a newer save is kept). */
  dismissRecipeNotice = (notice?: RecipeNotice) => {
    if (!notice || this.recipeNotice === notice) this.recipeNotice = null;
  };
  /** Closes the listen-counting notice. */
  hideClickNotice = () => (this.clickNotice = false);

  isFavorite = (id: string) => this.favorites.some((f) => f.station_id === id);

  /** Call once on mount; returns the unsubscribe function. */
  async init(): Promise<() => void> {
    // Subscribed together; one that fails must not leak the others nor stop the loading below.
    const subscriptions = await Promise.allSettled([
      listen('player:state', (e) => {
        this.state = e.payload;
        if (e.payload.status !== 'error') this.goneFavorite = null;
        for (const watch of [...this.stateWatchers]) watch();
      }),
      listen('favorite:gone', (e) => (this.goneFavorite = e.payload)),
      listen('player:now-playing', (e) => (this.nowPlaying = e.payload)),
      listen('player:cover', (e) => (this.cover = e.payload)),
      listen('player:muted', (e) => (this.muted = e.payload)),
      listen('player:title-status', (e) => (this.titleStatus = e.payload)),
      listen('recipes:notice', (e) => (this.recipeNotice = e.payload)),
      // A favorite that had moved now plays from its new address, saved by the backend.
      listen('favorites:changed', () =>
        this.run(async () => (this.favorites = await api.listFavorites())),
      ),
      // Station switched from the system media controls (next / previous favorite)
      listen('player:station', (e) => {
        const { show_click_notice, skipped, ...station } = e.payload;
        // The same station again is a retry at its new address: what was skipped still stands.
        if (station.id !== this.current?.id) this.skipped = skipped;
        this.current = station;
        if (show_click_notice) this.clickNotice = true;
      }),
    ]);
    const offs = subscriptions.flatMap((r) => (r.status === 'fulfilled' ? [r.value] : []));
    await this.run(async () => {
      // Independent reads, sent together rather than one round trip after another.
      const [state, nowPlaying, cover, titleStatus, favorites, volume, current, muted] =
        await Promise.all([
          api.getPlayerState(),
          api.getNowPlaying(),
          api.getCover(),
          api.getTitleStatus(),
          api.listFavorites(),
          api.getVolume(),
          // The last station played, in this run (app kept playing in the background) or the previous one.
          api.getCurrentStation(),
          api.getMuted(),
        ]);
      this.state = state;
      this.nowPlaying = nowPlaying;
      this.cover = cover;
      this.titleStatus = titleStatus;
      this.favorites = favorites;
      this.current = current;
      this.volume = volume ?? this.volume;
      this.muted = muted;
      await api.setVolume(this.volume);
    });
    // Reported after the loading, which clears the player's error when it starts.
    const failed = subscriptions.find((r) => r.status === 'rejected');
    if (failed) errors.report('player', failed.reason);
    return () => offs.forEach((off) => off());
  }

  async play(station: Playable) {
    this.current = station;
    this.skipped = [];
    await this.run(async () => {
      const outcome = await api.playStream(station.url, station.id, station.name);
      if (outcome.show_click_notice) this.clickNotice = true;
    });
  }

  /** Surprise me (docs/10-discovery.md §4.1): plays a random music station that is actually
   *  heard. Candidates come from the backend (healthy, tagged as music, not talk); each one gets
   *  a few seconds to sound, otherwise the next is tried, silently. Choosing a station meanwhile
   *  stops the search. */
  async surprise() {
    if (this.surprising) return;
    this.surprising = true;
    try {
      for (let draw = 0; draw < SURPRISE_DRAWS; draw++) {
        const found = await api.surpriseCandidates(this.current?.id ?? null);
        for (const s of found) {
          if (this.surprised.has(s.id)) continue;
          this.surprised.add(s.id);
          const station = playableOf(s);
          await this.play(station);
          const heard = await this.heard();
          if (this.current?.id !== station.id) return; // the listener chose another station
          if (heard) return;
        }
      }
      await this.stop();
      errors.report('player', 'Nothing playable came up. Try Surprise me again.');
    } catch (e) {
      errors.report('player', e);
    } finally {
      this.surprising = false;
    }
  }

  /** Called on each state the backend sends (`heard` waits on them). */
  private stateWatchers = new Set<() => void>();

  /** Waits until the station just started is heard (true), fails or stays silent (false). */
  private heard(): Promise<boolean> {
    return new Promise((resolve) => {
      const settle = (heard: boolean) => {
        clearTimeout(timer);
        this.stateWatchers.delete(check);
        resolve(heard);
      };
      const check = () => {
        const status = this.state.status;
        if (status === 'playing' || status === 'paused') settle(true);
        // Reconnecting right after a start means the stream failed and the engine retries:
        // for a surprise, the next station is quicker.
        else if (status === 'error' || status === 'reconnecting') settle(false);
      };
      const timer = setTimeout(() => settle(false), SURPRISE_WAIT_MS);
      this.stateWatchers.add(check);
      check();
    });
  }

  async stop() {
    await this.run(() => api.stopStream());
  }

  /** The main button, as the system media keys: pause and resume the stream. Connecting
   *  cannot be paused yet, so it stops; after a stop or an error, the station plays again. */
  async toggle() {
    switch (this.state.status) {
      case 'playing':
        return this.run(() => api.pauseStream());
      case 'paused':
        return this.run(() => api.resumeStream());
      case 'connecting':
      case 'reconnecting':
        return this.stop();
      default:
        if (this.current) await this.play(this.current);
    }
  }

  /** Moving the slider also brings the sound back after a mute. */
  async setVolume(volume: number) {
    this.volume = Math.min(1, Math.max(0, volume));
    await this.run(async () => {
      await api.setVolume(this.volume);
      if (this.muted) await api.setMuted(false);
    }, KEEP_ERROR);
  }

  async toggleMute() {
    await this.run(() => api.setMuted(!this.muted), KEEP_ERROR);
  }

  /** Remembers the current volume for the next launch (once the slider is released). */
  async saveVolume() {
    await this.run(() => api.saveVolume(this.volume), KEEP_ERROR);
  }

  /** Whether it was done: a failure is shown in the player. */
  async toggleFavorite(station: Playable): Promise<boolean> {
    return this.run(async () => {
      if (this.isFavorite(station.id)) {
        await api.removeFavorite(station.id);
      } else {
        await api.addFavorite({
          station_id: station.id,
          name: station.name,
          stream_url: station.url,
          genre: station.genre ?? null,
        });
      }
      this.favorites = await api.listFavorites();
    });
  }

  /** Puts the favorites in this order (station ids, top first), shown at once. */
  async reorderFavorites(ids: string[]) {
    const byId = new Map(this.favorites.map((f) => [f.station_id, f]));
    this.favorites = ids.flatMap((id) => byId.get(id) ?? []);
    await this.run(async () => {
      try {
        await api.reorderFavorites(ids);
      } finally {
        this.favorites = await api.listFavorites();
      }
    });
  }

  /** Answer to "Remove it from your favorites?" after a favorite vanished from the catalog. */
  async answerGoneFavorite(remove: boolean) {
    const id = this.goneFavorite;
    this.goneFavorite = null;
    if (!remove || !id) return;
    await this.run(async () => {
      await api.removeFavorite(id);
      this.favorites = await api.listFavorites();
    });
  }

  /** Runs a player command, its failure shown in the player. A new command clears the last
   *  error, except sound adjustments (`KEEP_ERROR`): a volume step fixes nothing, and the
   *  slider and the wheel would erase a message before it is read. */
  private async run(action: () => Promise<unknown>, keepError = false): Promise<boolean> {
    if (!keepError) errors.clear('player');
    try {
      await action();
      return true;
    } catch (e) {
      errors.report('player', e);
      return false;
    }
  }
}

export const player = new PlayerStore();
