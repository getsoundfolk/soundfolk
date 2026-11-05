// SPDX-License-Identifier: MPL-2.0
// Likes shared between the player and the History tab.
import { api, errorMessage, type HistoryEntry } from './api';
import { player } from './player.svelte';

class HistoryStore {
  /** Last logged title, if it is the one on air now (the player's ♥ applies to it). */
  latest = $state<HistoryEntry | null>(null);
  /** Bumped after every like change: lists depending on likes reload. */
  version = $state(0);

  /** Title on air as logged in the history, or null (not logged, e.g. the station's own name). */
  onAir = $derived(
    this.latest &&
      this.latest.track_title === player.nowPlaying.title?.trim() &&
      this.latest.station_id === player.current?.id
      ? this.latest
      : null,
  );

  async refreshLatest() {
    try {
      this.latest = (await api.getListeningHistory(1))[0] ?? null;
    } catch (e) {
      player.error = errorMessage(e);
    }
  }

  /** Likes a history entry's track, or unlikes a track by its key. */
  async toggleLike(track: { track_key: string; liked: boolean; historyId?: number }) {
    try {
      if (track.liked) await api.unlikeTrack(track.track_key);
      else if (track.historyId !== undefined) await api.likeTrack(track.historyId);
      if (this.latest?.track_key === track.track_key)
        this.latest = { ...this.latest, liked: !track.liked };
      this.version++;
    } catch (e) {
      player.error = errorMessage(e);
    }
  }
}

export const history = new HistoryStore();
