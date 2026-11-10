<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { focusFirstItem, menuKeydown } from '../a11y';
  import { api } from '../api';
  import { errors } from '../errors.svelte';
  import { player } from '../player.svelte';
  import { share } from '../share.svelte';
  import { displayTitle, shareText } from '../trackLinks';
  import { trackServices } from '../trackServices.svelte';
  import Icon from './Icon.svelte';
  import styles from './TrackMenu.module.css';

  // The ⋯ button of a History row and its menu: find the track, copy or share it, play the
  // station or keep it. One menu is open at a time: the panel says which (`open`).
  let {
    track,
    open,
    ontoggle,
    onclose,
    onnotify,
  }: {
    track: {
      artist: string | null;
      title: string;
      station_id: string;
      station_name: string | null;
      search_query: string;
    };
    open: boolean;
    ontoggle: () => void;
    onclose: () => void;
    /** A short message for the panel's toast. */
    onnotify: (message: string) => void;
  } = $props();

  let button = $state<HTMLButtonElement>();
  let menu = $state<HTMLElement>();
  /**
   * Where the menu shows. It is fixed to the window so the scrolling panel cannot clip it, and
   * opens on the side of the button with more room.
   */
  let placement = $state('');

  function toggle() {
    if (!open && button) {
      const rect = button.getBoundingClientRect();
      const right = `right: ${window.innerWidth - rect.right}px;`;
      const below = window.innerHeight - rect.bottom;
      placement =
        below >= rect.top
          ? `${right} top: ${rect.bottom + 4}px; max-height: ${below - 12}px;`
          : `${right} bottom: ${window.innerHeight - rect.top + 4}px; max-height: ${rect.top - 12}px;`;
    }
    ontoggle();
  }

  /** Closes the menu; from the keyboard or an action, the focus goes back to its button. */
  function close(refocus: boolean) {
    if (!open) return;
    onclose();
    if (refocus && button?.isConnected) button.focus();
  }

  // While open: Escape, a click elsewhere, a resize or a scroll close it. A fixed menu would stay
  // put while its row scrolls away; a scroll of the menu itself (it scrolls when the window is
  // too short to show all its items) does not count.
  $effect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close(true);
    const onClick = (e: MouseEvent) =>
      !button?.parentElement?.contains(e.target as Node) && close(false);
    const onResize = () => close(false);
    const onScroll = (e: Event) => !menu?.contains(e.target as Node) && close(false);
    window.addEventListener('keydown', onKey);
    window.addEventListener('click', onClick);
    window.addEventListener('resize', onResize);
    document.addEventListener('scroll', onScroll, { capture: true, passive: true });
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('click', onClick);
      window.removeEventListener('resize', onResize);
      document.removeEventListener('scroll', onScroll, { capture: true });
    };
  });

  /**
   * Runs a menu action: closes the menu, reports a failure. `action` may answer `false`: it
   * failed and showed why elsewhere (the player), so `done` is not said.
   */
  async function act(action: () => Promise<unknown>, done?: string) {
    close(true);
    errors.clear('history');
    try {
      const outcome = await action();
      if (done && outcome !== false) onnotify(done);
    } catch (e) {
      errors.report('history', e);
    }
  }

  const copy = (text: string, done: string) => act(() => writeText(text), done);
  const resolve = () => api.resolveStation(track.station_id, track.station_name);
</script>

<div class={styles.actions}>
  <button
    class={styles.more}
    type="button"
    bind:this={button}
    aria-label={`Actions for ${track.title}`}
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={toggle}><Icon name="more" /></button
  >
  {#if open}
    <div
      class={styles.menu}
      style={placement}
      role="menu"
      tabindex="-1"
      aria-label={`Actions for ${track.title}`}
      bind:this={menu}
      onkeydown={menuKeydown}
      onfocusout={(e) =>
        !e.currentTarget.contains(e.relatedTarget as Node | null) &&
        e.relatedTarget !== button &&
        close(false)}
      {@attach focusFirstItem}
    >
      {#if trackServices.enabled.length}
        <p class={styles.heading}>Find this track on</p>
        {#each trackServices.enabled as service (service.id)}
          <button
            class={styles.item}
            type="button"
            role="menuitem"
            tabindex="-1"
            onclick={() => act(() => openUrl(service.url(track.search_query)))}
          >
            {service.name}
          </button>
        {/each}
        <hr class={styles.separator} />
      {/if}
      <button
        class={styles.item}
        type="button"
        role="menuitem"
        tabindex="-1"
        onclick={() => copy(displayTitle(track), 'Title copied')}
      >
        Copy title
      </button>
      <button
        class={styles.item}
        type="button"
        role="menuitem"
        tabindex="-1"
        onclick={() =>
          copy(shareText(displayTitle(track), track.station_name), 'Copied, ready to share')}
      >
        Copy to share
      </button>
      <button
        class={styles.item}
        type="button"
        role="menuitem"
        tabindex="-1"
        onclick={() => act(async () => share.offerStation(await resolve(), displayTitle(track)))}
      >
        Share link…
      </button>
      <hr class={styles.separator} />
      <button
        class={styles.item}
        type="button"
        role="menuitem"
        tabindex="-1"
        onclick={() => act(async () => player.play(await resolve()))}
      >
        Play {track.station_name ?? 'the station'}
      </button>
      {#if player.isFavorite(track.station_id)}
        <button
          class={styles.item}
          type="button"
          role="menuitem"
          tabindex="-1"
          onclick={() =>
            act(
              () =>
                player.toggleFavorite({
                  id: track.station_id,
                  name: track.station_name ?? '',
                  url: '',
                }),
              'Station removed from favorites',
            )}
        >
          Remove station from favorites
        </button>
      {:else}
        <button
          class={styles.item}
          type="button"
          role="menuitem"
          tabindex="-1"
          onclick={() =>
            act(async () => player.toggleFavorite(await resolve()), 'Station added to favorites')}
        >
          Add station to favorites
        </button>
      {/if}
    </div>
  {/if}
</div>
