<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount } from 'svelte';
  import { api } from './lib/api';
  import ClickNotice from './lib/components/ClickNotice.svelte';
  import SettingsPage from './lib/components/SettingsPage.svelte';
  import PlayerControls from './lib/components/PlayerControls.svelte';
  import RecipeNotice from './lib/components/RecipeNotice.svelte';
  import StationSheet from './lib/components/StationSheet.svelte';
  import StationBrowser from './lib/components/StationBrowser.svelte';
  import { history } from './lib/history.svelte';
  import { lists } from './lib/lists.svelte';
  import { player } from './lib/player.svelte';
  import { ui } from './lib/ui.svelte';

  onMount(() => {
    const ready = player.init();
    const listsReady = lists.init();
    // Back from the editor: a recipe saved meanwhile is picked up now (the folder is not polled).
    const focus = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) api.reloadRecipes().catch(() => {});
    });
    return () => {
      ready.then((off) => off());
      listsReady.then((off) => off());
      focus.then((off) => off());
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
  <StationSheet />
  <RecipeNotice />
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
