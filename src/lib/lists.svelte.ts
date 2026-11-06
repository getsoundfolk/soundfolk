// SPDX-License-Identifier: MPL-2.0
// Curated lists (docs/14-curated-lists.md): loaded once, again when a new file was downloaded, and
// the lists the station in the player is in.
import { listen } from '@tauri-apps/api/event';
import { api, type CuratedList, type ListRef, type Playable } from './api';

// By title, as the listener's language sorts them: no list ranks above another (14 §6.3).
const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });

class ListsStore {
  all = $state<CuratedList[]>([]);
  /** The lists the station in the player is in, in the order of `all`. */
  forCurrent = $state<ListRef[]>([]);
  /** Bumped when a new file was downloaded. */
  version = $state(0);

  get = (id: string) => this.all.find((l) => l.id === id) ?? null;

  /** Call once on mount; returns the unsubscribe function. */
  async init(): Promise<() => void> {
    const off = await listen('lists:updated', () => {
      this.version++;
      this.load();
    }).catch(() => () => {});
    await this.load();
    return off;
  }

  private async load() {
    try {
      const lists = await api.getCuratedLists();
      this.all = lists.sort((a, b) => collator.compare(a.title, b.title));
    } catch {
      // Lists are an extra: without them the app works as before.
    }
  }

  /** Asks which lists this station is in; a late answer for another station is dropped. */
  private asked: Playable | null = null;
  async refreshFor(station: Playable | null) {
    this.asked = station;
    if (!station) {
      this.forCurrent = [];
      return;
    }
    try {
      const refs = await api.getListsForStation(station.id || null, station.url);
      if (this.asked !== station) return;
      const order = new Map(this.all.map((l, i) => [l.id, i]));
      this.forCurrent = refs.sort((a, b) => (order.get(a.id) ?? 0) - (order.get(b.id) ?? 0));
    } catch {
      if (this.asked === station) this.forCurrent = [];
    }
  }
}

export const lists = new ListsStore();
