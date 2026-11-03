<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount } from 'svelte';
  import ClickNotice from './lib/components/ClickNotice.svelte';
  import PlayerControls from './lib/components/PlayerControls.svelte';
  import StationBrowser from './lib/components/StationBrowser.svelte';
  import { history } from './lib/history.svelte';
  import { player } from './lib/player.svelte';

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
  <ClickNotice />
  <PlayerControls />
  <StationBrowser />
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    height: 100vh;
    padding: 0.75rem;
  }
</style>
