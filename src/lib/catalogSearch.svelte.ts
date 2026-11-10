// SPDX-License-Identifier: MPL-2.0
import { api, errorMessage, type Station } from './api';

const PAGE = 30;
/** Pause in typing before the catalog is queried, so it is not hit on every keystroke. */
const DEBOUNCE_MS = 300;

/**
 * The online catalog search of Discover. It outlives the panel: back on the tab, the text and
 * the results are still there.
 */
class CatalogSearch {
  text = $state('');
  results = $state<Station[]>([]);
  loading = $state(false);
  error = $state<string | null>(null);
  // Offset of the next page, or null when the catalog has no more results.
  private nextOffset = $state<number | null>(null);
  readonly hasMore = $derived(this.nextOffset !== null);
  // Id of the latest request: an older response is ignored.
  private requestId = 0;
  // Text of the results shown, or null before the first search and after a failure (retried).
  private searched: string | null = null;

  /** The text typed; [`follow`] searches it. */
  setText = (text: string) => (this.text = text);

  /**
   * Searches the text typed, once typing pauses; for an `$effect`, which it gives the cleanup.
   * The same text is not searched again; without text, nothing is shown.
   */
  follow(): (() => void) | undefined {
    const query = this.text.trim();
    if (query === this.searched) return;
    if (query === '') {
      ++this.requestId;
      this.searched = '';
      this.results = [];
      this.nextOffset = null;
      this.loading = false;
      this.error = null;
      return;
    }
    const timer = setTimeout(() => {
      this.searched = query;
      this.search(true);
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }

  /** The first page of results (`reset`), or the next one. */
  async search(reset: boolean) {
    const mine = ++this.requestId;
    this.loading = true;
    this.error = null;
    try {
      const page = await api.searchStations({
        text: this.text.trim() || undefined,
        limit: PAGE,
        // The offset counts the catalog's own entries, not the stations shown (some are filtered out).
        offset: reset ? 0 : (this.nextOffset ?? 0),
      });
      if (mine !== this.requestId) return;
      this.results = reset ? page.stations : [...this.results, ...page.stations];
      this.nextOffset = page.next_offset;
    } catch (e) {
      if (mine === this.requestId) {
        this.error = errorMessage(e);
        this.searched = null;
      }
    } finally {
      if (mine === this.requestId) this.loading = false;
    }
  }
}

// Read-only outside the class: its fields change only through its methods (docs/99 §2.5).
export const catalogSearch: Readonly<CatalogSearch> = new CatalogSearch();
