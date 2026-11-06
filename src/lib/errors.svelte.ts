// SPDX-License-Identifier: MPL-2.0
// Command errors, one per area of the window: each is shown where its action was taken, and
// only an action of the same area (or a dismiss) clears it. Playback errors arrive via the
// player state instead.
import { errorMessage } from './api';

export type ErrorArea = 'player' | 'stations' | 'history' | 'settings';

class ErrorStore {
  /** The player's own commands: play, pause, volume, favorites, the listen-counting notice. */
  player = $state<string | null>(null);
  /** The station lists: personal stations. */
  stations = $state<string | null>(null);
  /** The History tab: lists, likes, menu actions, export, stats. */
  history = $state<string | null>(null);
  /** The settings page. */
  settings = $state<string | null>(null);

  report = (area: ErrorArea, e: unknown) => (this[area] = errorMessage(e));
  clear = (area: ErrorArea) => (this[area] = null);
}

export const errors = new ErrorStore();
