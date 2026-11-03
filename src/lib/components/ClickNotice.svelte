<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { api, errorMessage } from '../api';
  import { player } from '../player.svelte';
  import styles from './ClickNotice.module.css';

  async function disable() {
    try {
      await api.setClickCounting(false);
      player.clickNotice = false;
    } catch (e) {
      player.error = errorMessage(e);
    }
  }
</script>

{#if player.clickNotice}
  <aside class={styles.notice} aria-label="Information about listen counting">
    <p class={styles.text}>
      This listen is reported to Radio-Browser (station + IP) to help independent radios.
    </p>
    <div class={styles.actions}>
      <button class={styles.primary} type="button" onclick={() => (player.clickNotice = false)}>OK</button>
      <button class={styles.secondary} type="button" onclick={disable}>Turn off</button>
    </div>
  </aside>
{/if}
