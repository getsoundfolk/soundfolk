// SPDX-License-Identifier: MPL-2.0
// Curated lists (docs/15-curated-lists.md): loaded once, again when a new file was downloaded, the
// lists the station in the player is in, and the lists marked as favorites.
import { unsubscribed } from './errors.svelte';
import { api, listen, type CuratedList, type ListRef, type Playable, type Station } from './api';

// By title, as the listener's language sorts them: no list ranks above another (14 §6.3).
const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });

class ListsStore {
  all = $state<CuratedList[]>([]);
  /** The lists the station in the player is in, in the order of `all`. */
  forCurrent = $state<ListRef[]>([]);
  /** Bumped when a new file was downloaded. */
  version = $state(0);
  /** Ids of the lists marked as favorites, in the order they were marked. */
  favoriteIds = $state<string[]>([]);

  get = (id: string) => this.all.find((l) => l.id === id) ?? null;
  isFavorite = (id: string) => this.favoriteIds.includes(id);

  /** The favorite lists still published, most recently marked first, like the favorite stations. */
  get favorites(): CuratedList[] {
    return this.favoriteIds
      .map((id) => this.get(id))
      .filter((l): l is CuratedList => l !== null)
      .reverse();
  }

  /** Call once on mount; returns the unsubscribe function. */
  async init(): Promise<() => void> {
    const off = await listen('lists:updated', () => {
      this.version++;
      this.stationsCache.clear();
      this.load();
    }).catch(unsubscribed);
    await Promise.all([
      this.load(),
      api
        .getFavoriteLists()
        .then((ids) => (this.favoriteIds = ids))
        .catch(() => {}),
    ]);
    return off;
  }

  /** Reads the lists again (after a download asked by a link). */
  reload = () => this.load();

  private async load() {
    try {
      const lists = await api.getCuratedLists();
      this.all = lists.sort((a, b) => collator.compare(a.title, b.title));
    } catch {
      // Lists are an extra: without them the app works as before.
    }
  }

  async toggleFavorite(id: string) {
    this.favoriteIds = await api.setFavoriteList(id, !this.isFavorite(id));
  }

  /** A list's stations from the catalog, asked once per list until a new file arrives. */
  private stationsCache = new Map<string, Promise<Station[]>>();
  stations(id: string): Promise<Station[]> {
    let found = this.stationsCache.get(id);
    if (!found) {
      found = api.getCuratedListStations(id);
      // A failure is not kept: the next visit asks again.
      found.catch(() => this.stationsCache.delete(id));
      this.stationsCache.set(id, found);
    }
    return found;
  }

  /** Lists whose title, description or curator contain `text`, in the order of `all`. */
  matching(text: string): CuratedList[] {
    const words = text.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
    if (words.length === 0) return [];
    return this.all.filter((l) => {
      const hay = [l.title, l.description, ...l.curators].join(' ').toLocaleLowerCase();
      return words.every((w) => hay.includes(w));
    });
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
