// SPDX-License-Identifier: MPL-2.0
// Shared player state (runes): a single subscription to the backend events.
import { listen } from '@tauri-apps/api/event';
import {
  api,
  errorMessage,
  type Favorite,
  type NowPlaying,
  type Playable,
  type PlayerState,
} from './api';

class PlayerStore {
  state = $state<PlayerState>({ status: 'idle' });
  nowPlaying = $state<NowPlaying>({ station: null, title: null });
  current = $state<Playable | null>(null);
  volume = $state(0.8);
  favorites = $state<Favorite[]>([]);
  /** Error from a command (not a playback error, which arrives via `state`). */
  error = $state<string | null>(null);
  clickNotice = $state(false);

  /** Audio is (or is about to be) playing: the main button becomes Stop. */
  active = $derived(this.state.status !== 'idle' && this.state.status !== 'error');

  isFavorite = (id: string) => this.favorites.some((f) => f.station_id === id);

  /** Call once on mount; returns the unsubscribe function. */
  async init(): Promise<() => void> {
    const offs = [
      await listen<PlayerState>('player:state', (e) => (this.state = e.payload)),
      await listen<NowPlaying>('player:now-playing', (e) => (this.nowPlaying = e.payload)),
    ];
    await this.run(async () => {
      this.state = await api.getPlayerState();
      this.nowPlaying = await api.getNowPlaying();
      this.favorites = await api.listFavorites();
      await api.setVolume(this.volume);
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

  async toggle() {
    if (this.active) await this.stop();
    else if (this.current) await this.play(this.current);
  }

  async setVolume(volume: number) {
    this.volume = volume;
    await this.run(() => api.setVolume(volume));
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
