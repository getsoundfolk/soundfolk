// SPDX-License-Identifier: MPL-2.0
// A newer version of the app (desktop only, src-tauri/src/updates.rs): a dot on the settings gear,
// and a line in Settings › About with its release page. Nothing during listening, nothing modal.
import { unsubscribed } from './errors.svelte';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api, listen, type Release } from './api';

class UpdatesStore {
  available = $state<Release | null>(null);

  /** Call once on mount; returns the unsubscribe function. */
  async init(): Promise<() => void> {
    const off = await listen('updates:available', () => this.load()).catch(unsubscribed);
    await this.load();
    return off;
  }

  /** Asks again (after a download, or the check switched on or off). */
  load = async () => {
    // eslint-disable-next-line no-restricted-syntax -- on mobile the command does not exist: no notice
    this.available = await api.getUpdate().catch(() => null);
  };

  open = () => (this.available ? openUrl(this.available.url) : Promise.resolve());

  /** No more notice for this version; the next one has one again. */
  skip = async () => {
    const release = this.available;
    if (!release) return;
    this.available = null;
    await api.skipUpdate(release.version);
  };
}

// Read-only outside the class: its fields change only through its methods (docs/99 §2.5).
export const updates: Readonly<UpdatesStore> = new UpdatesStore();
