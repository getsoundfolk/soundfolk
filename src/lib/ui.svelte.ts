// SPDX-License-Identifier: MPL-2.0
// Which screen the window shows. Settings cover the whole window; the home screen stays mounted
// underneath so the search, the history filters and the scroll positions survive a visit to the settings.

class UiStore {
  settingsOpen = $state(false);
  /** The station sheet (details of the station in the player), shown over the home screen. */
  stationOpen = $state(false);

  openSettings = () => (this.settingsOpen = true);
  closeSettings = () => (this.settingsOpen = false);
  openStation = () => (this.stationOpen = true);
  closeStation = () => (this.stationOpen = false);
}

export const ui = new UiStore();
