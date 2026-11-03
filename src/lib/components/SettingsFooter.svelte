<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage } from '../api';
  import { player } from '../player.svelte';
  import styles from './SettingsFooter.module.css';

  let clickCounting = $state(true);

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

<footer class={styles.footer}>
  <label class={styles.label}>
    <input
      type="checkbox"
      checked={clickCounting}
      onchange={(e) => change(e.currentTarget.checked)}
    />
    Report my listens to Radio-Browser (station + IP) to help independent radios
  </label>
</footer>
