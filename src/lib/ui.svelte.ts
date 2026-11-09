// SPDX-License-Identifier: MPL-2.0
// Which screen the window shows. Settings cover the whole window; the home screen stays mounted
// underneath so the search, the history filters and the scroll positions survive a visit to the settings.
import type { Station } from './api';

class UiStore {
  settingsOpen = $state(false);
  /** The station sheet, shown over the home screen. */
  stationOpen = $state(false);
  /** The catalog station the sheet shows, or null for the station in the player. */
  sheetStation = $state<Station | null>(null);

  openSettings = () => (this.settingsOpen = true);
  closeSettings = () => (this.settingsOpen = false);
  /** The curated list shown in Listen or Discover, or null for the tab itself. */
  listId = $state<string | null>(null);

  /** Opens the sheet on the station in the player. */
  openStation = () => {
    this.sheetStation = null;
    this.stationOpen = true;
  };
  /** Opens the sheet on a station from a list, to look at it before playing or liking it. */
  showStation = (station: Station) => {
    this.sheetStation = station;
    this.stationOpen = true;
  };
  closeStation = () => (this.stationOpen = false);
  /** Shows a list in the current tab, over whatever covered the home screen. */
  openList = (id: string | null) => {
    this.stationOpen = false;
    this.settingsOpen = false;
    this.listId = id;
    if (id) this.listRequest++;
  };
  /** Bumped when a list is opened: the station browser shows it in the current tab. */
  listRequest = $state(0);
  /** Ctrl+F / Ctrl+K: back to the home screen, the search field focused (station browser). */
  focusSearch = () => {
    this.openList(null);
    this.searchRequest++;
  };
  searchRequest = $state(0);
  /** Bumped when a personal station was added elsewhere than in My stations (a share link).
   *  Read it; change it through `personalAdded`. */
  personalChanged = $state(0);
  /** A personal station was added elsewhere than in My stations: My stations reloads. */
  personalAdded = () => {
    this.personalChanged++;
  };
}

export const ui = new UiStore();
