// SPDX-License-Identifier: MPL-2.0
// Names the user gave stations, shown instead of the catalog's everywhere in the window. The
// backend applies them to the history, the likes, the stats and the media controls.
import { api } from './api';
import { history } from './history.svelte';

class StationNamesStore {
  /** By station ID. */
  names = $state<Record<string, string>>({});

  /** Call once on mount. A failure keeps the stations' own names. */
  async init() {
    try {
      this.names = await api.getStationNames();
    } catch {
      // Not worth an error banner: every station keeps its own name.
    }
  }

  /** The name the user gave the station, else `own`. */
  shown = (id: string | null | undefined, own: string) => (id && this.names[id]) || own;

  /** The name the user gave the station, if any. */
  given = (id: string | null | undefined) => (id ? (this.names[id] ?? null) : null);

  /** Names a station; a blank name gives it back its own. Throws when it was not saved. */
  async rename(id: string, name: string) {
    const kept = await api.setStationName(id, name.trim() || null);
    const names = { ...this.names };
    if (kept) names[id] = kept;
    else delete names[id];
    this.names = names;
    // The history, the likes and the stats show the new name.
    history.version++;
  }
}

export const stationNames = new StationNamesStore();
