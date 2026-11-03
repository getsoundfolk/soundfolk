<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage } from '../api';
  import { player } from '../player.svelte';
  import styles from './SettingsMenu.module.css';

  let open = $state(false);
  let clickCounting = $state(true);
  let root: HTMLDivElement | undefined = $state();

  onMount(async () => {
    try {
      clickCounting = await api.getClickCounting();
    } catch (e) {
      player.error = errorMessage(e);
    }
  });

  async function change(enabled: boolean) {
    clickCounting = enabled;
    try {
      await api.setClickCounting(enabled);
      if (!enabled) player.clickNotice = false;
    } catch (e) {
      clickCounting = !enabled;
      player.error = errorMessage(e);
    }
  }
</script>

<!-- Close on Escape or on a click outside the menu. -->
<svelte:window
  onkeydown={(e) => e.key === 'Escape' && (open = false)}
  onclick={(e) => open && root && !root.contains(e.target as Node) && (open = false)}
/>

<div class={styles.root} bind:this={root}>
  <button
    class={styles.gear}
    type="button"
    aria-label="Settings"
    aria-expanded={open}
    aria-controls="settings-panel"
    onclick={() => (open = !open)}>⚙&#xFE0E;</button
  >

  {#if open}
    <div class={styles.panel} id="settings-panel" role="group" aria-label="Settings">
      <label class={styles.label}>
        <input
          type="checkbox"
          checked={clickCounting}
          onchange={(e) => change(e.currentTarget.checked)}
        />
        Report my listens to Radio-Browser (station + IP) to help radios get discovered
      </label>
    </div>
  {/if}
</div>
