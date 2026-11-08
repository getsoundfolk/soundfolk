// SPDX-License-Identifier: MPL-2.0
// Share links (docs/16-sharing.md): links received (pasted, dropped, opened from the page) and
// links made to send. A link is something someone else chose: it opens a sheet, it never plays
// or saves anything by itself (§3.3).
import { listen } from '@tauri-apps/api/event';
import {
  api,
  errorMessage,
  type Playable,
  type ShareOut,
  type Shared,
  type SharedLink,
} from './api';
import { lists } from './lists.svelte';
import { ui } from './ui.svelte';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
/** What may be a share link, before asking the backend: a web or `soundfolk://` address, or a
 *  bare station UUID. */
const LIKE_A_LINK = /^\s*(?:(?:https?|soundfolk):\/\/\S+|[0-9a-f]{8}-[0-9a-f-]{27})\s*$/i;

/** What a station shares as: a catalog station by its UUID, any other by its name and stream. */
export const sharedOf = (s: Playable): Shared =>
  UUID.test(s.id)
    ? { kind: 'station', id: s.id.toLowerCase() }
    : { kind: 'personal_station', name: s.name, url: s.url };

/** A received link the shared sheet shows: everything but a catalog station or a known list,
 *  which open the station sheet or the list's screen. */
export type Received =
  | { kind: 'invalid'; message: string }
  | { kind: 'gone'; link: SharedLink & { kind: 'station' } }
  | { kind: 'unknown_list'; link: SharedLink & { kind: 'list' } }
  | { kind: 'personal_station'; link: SharedLink & { kind: 'personal_station' } }
  | { kind: 'personal_list'; link: SharedLink & { kind: 'personal_list' } };

/** The link being offered in the share panel. */
export interface Offer {
  /** What is shared, as the panel's title says it: a station's or a list's name. */
  name: string;
  track: string | null;
  out: ShareOut;
}

class ShareStore {
  /** The received link the shared sheet shows, or null. */
  received = $state<Received | null>(null);
  /** Sender and track of the link that opened the station sheet, for that station only. */
  note = $state<{ stationId: string; from: string | null; track: string | null } | null>(null);
  /** The share panel's link, or null when it is closed. */
  offer = $state<Offer | null>(null);
  /** Why the last link could not be made. */
  offerError = $state<string | null>(null);
  /** A link is being read or made. */
  busy = $state(false);

  /** Call once on mount: takes the `soundfolk://` link Soundfolk was opened with, and the ones
   *  opened later. Returns the unsubscribe function. */
  async init(): Promise<() => void> {
    const off = await listen('share:opened', () => this.takeOpened()).catch(() => () => {});
    await this.takeOpened();
    return off;
  }

  private async takeOpened() {
    const url = await api.takeOpenedLink().catch(() => null);
    if (url) await this.openText(url);
  }

  /** Whether a text may be a share link (cheap, no backend call). */
  mayBeLink = (text: string) => LIKE_A_LINK.test(text);

  /** Opens a pasted, dropped or opened text when it is a share link. False when it is not one
   *  (the text is then a search); a broken link shows why, and counts as handled. */
  async openText(text: string): Promise<boolean> {
    if (!this.mayBeLink(text)) return false;
    let link: SharedLink | null;
    try {
      link = await api.readShareLink(text);
    } catch (e) {
      this.show({ kind: 'invalid', message: errorMessage(e) });
      return true;
    }
    if (!link) return false;
    await this.open(link);
    return true;
  }

  private show(received: Received) {
    ui.closeStation();
    this.received = received;
  }

  closeReceived = () => (this.received = null);

  /** Opens what a link carries: a station's sheet, a list's screen, or the shared sheet. */
  async open(link: SharedLink) {
    this.busy = true;
    this.note = null;
    this.received = null;
    try {
      switch (link.kind) {
        case 'station': {
          const station = await api.getStation(link.id);
          if (!station) return this.show({ kind: 'gone', link });
          this.note = { stationId: station.id, from: link.from, track: link.track };
          ui.showStation(station);
          return;
        }
        case 'list': {
          // Published since the app's daily download: fetch the lists once more (§3.3).
          if (!lists.get(link.id)) {
            await api.refreshCuratedLists().catch(() => false);
            await lists.reload();
          }
          if (lists.get(link.id)) ui.openList(link.id);
          else this.show({ kind: 'unknown_list', link });
          return;
        }
        case 'personal_station':
          return this.show({ kind: 'personal_station', link });
        case 'personal_list':
          return this.show({ kind: 'personal_list', link });
      }
    } catch (e) {
      this.show({ kind: 'invalid', message: errorMessage(e) });
    } finally {
      this.busy = false;
    }
  }

  /** Opens the share panel on a station, with the track on air when there is one. */
  offerStation = (station: Playable, track: string | null = null) =>
    this.makeOffer(sharedOf(station), station.name, track);

  offerList = (id: string, title: string) => this.makeOffer({ kind: 'list', id }, title, null);

  private async makeOffer(item: Shared, name: string, track: string | null) {
    this.offerError = null;
    this.busy = true;
    try {
      this.offer = { name, track, out: await api.makeShareLink(item, track) };
    } catch (e) {
      this.offerError = errorMessage(e);
      this.offer = null;
    } finally {
      this.busy = false;
    }
  }

  closeOffer = () => {
    this.offer = null;
    this.offerError = null;
  };
}

export const share = new ShareStore();
