<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount } from 'svelte';
  import ClickNotice from './lib/components/ClickNotice.svelte';
  import PlayerControls from './lib/components/PlayerControls.svelte';
  import SettingsFooter from './lib/components/SettingsFooter.svelte';
  import StationBrowser from './lib/components/StationBrowser.svelte';
  import { player } from './lib/player.svelte';

  onMount(() => {
    const ready = player.init();
    return () => {
      ready.then((off) => off());
    };
  });
</script>

<main>
  <ClickNotice />
  <PlayerControls />
  <StationBrowser />
  <SettingsFooter />
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
