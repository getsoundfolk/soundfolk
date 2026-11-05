// SPDX-License-Identifier: MPL-2.0
// Shared player state (runes): a single subscription to the backend events.
import { listen } from '@tauri-apps/api/event';
import {
  api,
  errorMessage,
  type Cover,
  type Favorite,
  type NowPlaying,
  type Playable,
  type PlayerState,
  type RecipeNotice,
  type StationChange,
  type TitleStatus,
} from './api';

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
  /** Error from a command (not a playback error, which arrives via `state`). */
  error = $state<string | null>(null);
  clickNotice = $state(false);
  /** Where the title of the current playback comes from (station sheet). */
  titleStatus = $state<TitleStatus | null>(null);
  /** What a local recipe the user just saved reads (or why it is not used). */
  recipeNotice = $state<RecipeNotice | null>(null);
  /** Favorite that failed and is no longer in the catalog: the player offers to remove it. */
  goneFavorite = $state<string | null>(null);

  /** Audio is (or is about to be) playing: the main button shows Pause. */
  active = $derived(['connecting', 'reconnecting', 'playing'].includes(this.state.status));

  isFavorite = (id: string) => this.favorites.some((f) => f.station_id === id);

  /** Call once on mount; returns the unsubscribe function. */
  async init(): Promise<() => void> {
    const offs = [
      await listen<PlayerState>('player:state', (e) => {
        this.state = e.payload;
        if (e.payload.status !== 'error') this.goneFavorite = null;
      }),
      await listen<string>('favorite:gone', (e) => (this.goneFavorite = e.payload)),
      await listen<NowPlaying>('player:now-playing', (e) => (this.nowPlaying = e.payload)),
      await listen<Cover>('player:cover', (e) => (this.cover = e.payload)),
      await listen<TitleStatus>('player:title-status', (e) => (this.titleStatus = e.payload)),
      await listen<RecipeNotice>('recipes:notice', (e) => (this.recipeNotice = e.payload)),
      // A favorite that had moved now plays from its new address, saved by the backend.
      await listen('favorites:changed', () => this.run(async () => (this.favorites = await api.listFavorites()))),
      // Station switched from the system media controls (next / previous favorite)
      await listen<StationChange>('player:station', (e) => {
        const { show_click_notice, ...station } = e.payload;
        this.current = station;
        if (show_click_notice) this.clickNotice = true;
      }),
    ];
    await this.run(async () => {
      this.state = await api.getPlayerState();
      this.nowPlaying = await api.getNowPlaying();
      this.cover = await api.getCover();
      this.titleStatus = await api.getTitleStatus();
      this.favorites = await api.listFavorites();
      this.volume = (await api.getVolume()) ?? this.volume;
      await api.setVolume(this.volume);
      // The window may be opened again while the app kept playing in the background.
      this.current = await api.getCurrentStation();
    });
    return () => offs.forEach((off) => off());
  }

  async play(station: Playable) {
    this.current = station;
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
    this.error = null;
    try {
      await action();
    } catch (e) {
      this.error = errorMessage(e);
    }
  }
}

export const player = new PlayerStore();
