<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { api } from '../api';
  import { errors } from '../errors.svelte';
  import { player } from '../player.svelte';
  import styles from './ClickNotice.module.css';

  async function disable() {
    errors.clear('player');
    try {
      await api.setClickCounting(false);
      player.hideClickNotice();
    } catch (e) {
      errors.report('player', e);
    }
  }
</script>

{#if player.clickNotice}
  <aside class={styles.notice} aria-label="Information about listen counting">
    <p class={styles.text}>
      This listen is reported to Radio-Browser (station + IP) to help radios get discovered.
    </p>
    <div class={styles.actions}>
      <button class={styles.primary} type="button" onclick={player.hideClickNotice}>OK</button>
      <button class={styles.secondary} type="button" onclick={disable}>Turn off</button>
    </div>
  </aside>
{/if}
