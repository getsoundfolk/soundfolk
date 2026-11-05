// SPDX-License-Identifier: MPL-2.0
// Which services a track's "Find this track on" menu lists, shared by the menu and the settings.
import { api, errorMessage, type CustomService } from './api';
import { player } from './player.svelte';
import { customToService, DEFAULT_SERVICE_IDS, TRACK_SERVICES } from './trackLinks';

const isCustom = (id: string) => id.startsWith('custom:');

class TrackServicesStore {
  ids = $state<string[]>(DEFAULT_SERVICE_IDS);
  custom = $state<CustomService[]>([]);
  #loaded: Promise<void> | null = null;

  /** The catalog, then the user's own sites. */
  all = $derived([...TRACK_SERVICES, ...this.custom.map(customToService)]);
  /** Enabled services, in that order. Unknown ids (a service since removed) are ignored. */
  enabled = $derived(this.all.filter((s) => this.ids.includes(s.id)));
  /** Whether the catalog part of the selection is the default one (the user's own sites aside). */
  isDefault = $derived.by(() => {
    const catalog = this.ids.filter((id) => !isCustom(id));
    return (
      catalog.length === DEFAULT_SERVICE_IDS.length &&
      DEFAULT_SERVICE_IDS.every((id) => catalog.includes(id))
    );
  });

  load() {
    this.#loaded ??= Promise.all([api.getTrackServices(), api.getCustomTrackServices()])
      .then(([ids, custom]) => {
        if (ids) this.ids = ids;
        this.custom = custom;
      })
      .catch((e) => {
        this.#loaded = null;
        player.error = errorMessage(e);
      });
    return this.#loaded;
  }

  toggle = (id: string, on: boolean) =>
    this.saveIds(on ? [...this.ids.filter((i) => i !== id), id] : this.ids.filter((i) => i !== id));

  /** Restores the default catalog selection; the user's own sites keep their state. */
  reset = () => this.saveIds([...DEFAULT_SERVICE_IDS, ...this.ids.filter(isCustom)]);

  /** Adds a site, ticked. Throws the backend's message if it refuses the site. */
  async add(name: string, template: string) {
    const site: CustomService = {
      id: `custom:${crypto.randomUUID().slice(0, 8)}`,
      name: name.trim(),
      template: template.trim(),
    };
    const custom = [...this.custom, site];
    await api.setCustomTrackServices(custom);
    this.custom = custom;
    await this.saveIds([...this.ids, site.id]);
  }

  async remove(id: string) {
    // Untick first: if the second save fails, the site stays listed but off, never half-removed.
    await this.saveIds(this.ids.filter((i) => i !== id));
    const before = this.custom;
    this.custom = this.custom.filter((c) => c.id !== id);
    try {
      await api.setCustomTrackServices(this.custom);
    } catch (e) {
      this.custom = before;
      player.error = errorMessage(e);
    }
  }

  private async saveIds(ids: string[]) {
    const before = this.ids;
    this.ids = ids;
    try {
      await api.setTrackServices(ids);
    } catch (e) {
      this.ids = before;
      player.error = errorMessage(e);
    }
  }
}

export const trackServices = new TrackServicesStore();
