<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount } from 'svelte';
  import ClickNotice from './lib/components/ClickNotice.svelte';
  import SettingsPage from './lib/components/SettingsPage.svelte';
  import PlayerControls from './lib/components/PlayerControls.svelte';
  import StationBrowser from './lib/components/StationBrowser.svelte';
  import { history } from './lib/history.svelte';
  import { player } from './lib/player.svelte';
  import { ui } from './lib/ui.svelte';

  onMount(() => {
    const ready = player.init();
    return () => {
      ready.then((off) => off());
    };
  });

  // The backend logs a title before announcing it: the player's ♥ then finds it in the history.
  $effect(() => {
    void player.nowPlaying.title;
    history.refreshLatest();
  });
</script>

<main>
  <!-- Settings take the whole window; the home screen stays mounted (hidden) to keep its state. -->
  {#if ui.settingsOpen}<SettingsPage />{/if}
  <div class="home" hidden={ui.settingsOpen}>
    <ClickNotice />
    <PlayerControls />
    <StationBrowser />
  </div>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    height: 100vh;
    padding: 0.75rem;
  }

  .home {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    flex: 1;
    min-height: 0;
  }

  .home[hidden] {
    display: none;
  }
</style>
