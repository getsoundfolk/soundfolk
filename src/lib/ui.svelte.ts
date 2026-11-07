// SPDX-License-Identifier: MPL-2.0
// Which screen the window shows. Settings cover the whole window; the home screen stays mounted
// underneath so the search, the history filters and the scroll positions survive a visit to the settings.

class UiStore {
  settingsOpen = $state(false);
  /** The station sheet (details of the station in the player), shown over the home screen. */
  stationOpen = $state(false);

  openSettings = () => (this.settingsOpen = true);
  closeSettings = () => (this.settingsOpen = false);
  /** The curated list shown in Discover, or null for Discover itself. */
  listId = $state<string | null>(null);

  openStation = () => (this.stationOpen = true);
  closeStation = () => (this.stationOpen = false);
  /** Shows a list in Discover, over whatever covered the home screen. */
  openList = (id: string | null) => {
    this.stationOpen = false;
    this.settingsOpen = false;
    this.listId = id;
    if (id) this.listRequest++;
  };
  /** Bumped when a list is opened: the station browser switches to Discover. */
  listRequest = $state(0);
}

export const ui = new UiStore();
