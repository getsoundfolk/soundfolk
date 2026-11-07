// SPDX-License-Identifier: MPL-2.0
// Shared player state (runes): a single subscription to the backend events.
import { listen } from '@tauri-apps/api/event';
import {
  api,
  type Cover,
  type Favorite,
  type NowPlaying,
  type Playable,
  type PlayerState,
  type RecipeNotice,
  type Skipped,
  type StationChange,
  type TitleStatus,
} from './api';
import { errors } from './errors.svelte';

class PlayerStore {
  state = $state<PlayerState>({ status: 'idle' });
  nowPlaying = $state<NowPlaying>({ station: null, title: null });
  /** Cover art of the track on air, chosen by the backend from the sources set in ⚙. */
  cover = $state<Cover>({ title: null, url: null });
  current = $state<Playable | null>(null);
  volume = $state(0.8);
  /** Sound cut by the speaker button; the slider keeps its level to come back to. Not saved. */
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

  /** Audio is (or is about to be) playing: the main button shows Pause. */
  active = $derived(['connecting', 'reconnecting', 'playing'].includes(this.state.status));

  isFavorite = (id: string) => this.favorites.some((f) => f.station_id === id);

  /** Call once on mount; returns the unsubscribe function. */
  async init(): Promise<() => void> {
    // Subscribed together; one that fails must not leak the others nor stop the loading below.
    const subscriptions = await Promise.allSettled([
      listen<PlayerState>('player:state', (e) => {
        this.state = e.payload;
        if (e.payload.status !== 'error') this.goneFavorite = null;
      }),
      listen<string>('favorite:gone', (e) => (this.goneFavorite = e.payload)),
      listen<NowPlaying>('player:now-playing', (e) => (this.nowPlaying = e.payload)),
      listen<Cover>('player:cover', (e) => (this.cover = e.payload)),
      listen<TitleStatus>('player:title-status', (e) => (this.titleStatus = e.payload)),
      listen<RecipeNotice>('recipes:notice', (e) => (this.recipeNotice = e.payload)),
      // A favorite that had moved now plays from its new address, saved by the backend.
      listen('favorites:changed', () =>
        this.run(async () => (this.favorites = await api.listFavorites())),
      ),
      // Station switched from the system media controls (next / previous favorite)
      listen<StationChange>('player:station', (e) => {
        const { show_click_notice, skipped, ...station } = e.payload;
        // The same station again is a retry at its new address: what was skipped still stands.
        if (station.id !== this.current?.id) this.skipped = skipped ?? [];
        this.current = station;
        if (show_click_notice) this.clickNotice = true;
      }),
    ]);
    const offs = subscriptions.flatMap((r) => (r.status === 'fulfilled' ? [r.value] : []));
    await this.run(async () => {
      // Independent reads, sent together rather than one round trip after another.
      const [state, nowPlaying, cover, titleStatus, favorites, volume, current] = await Promise.all(
        [
          api.getPlayerState(),
          api.getNowPlaying(),
          api.getCover(),
          api.getTitleStatus(),
          api.listFavorites(),
          api.getVolume(),
          // The last station played, in this run (app kept playing in the background) or the previous one.
          api.getCurrentStation(),
        ],
      );
      this.state = state;
      this.nowPlaying = nowPlaying;
      this.cover = cover;
      this.titleStatus = titleStatus;
      this.favorites = favorites;
      this.current = current;
      this.volume = volume ?? this.volume;
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
    this.muted = false;
    await this.run(() => api.setVolume(this.volume));
  }

  async toggleMute() {
    this.muted = !this.muted;
    await this.run(() => api.setVolume(this.muted ? 0 : this.volume));
  }

  /** Remembers the current volume for the next launch (once the slider is released). */
  async saveVolume() {
    await this.run(() => api.saveVolume(this.volume));
  }

  async toggleFavorite(station: Playable) {
    await this.run(async () => {
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

  private async run(action: () => Promise<unknown>) {
    errors.clear('player');
    try {
      await action();
    } catch (e) {
      errors.report('player', e);
    }
  }
}

export const player = new PlayerStore();
