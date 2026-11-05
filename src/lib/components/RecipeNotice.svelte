<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- After a local recipe is saved: what it reads, or why it is not used. -->
<script lang="ts">
  import { player } from '../player.svelte';
  import { ui } from '../ui.svelte';
  import styles from './RecipeNotice.module.css';

  const DISPLAY_MS = 10_000;

  // An error stays until dismissed; a success fades after a while.
  $effect(() => {
    const notice = player.recipeNotice;
    if (!notice?.ok) return;
    const timer = setTimeout(() => {
      if (player.recipeNotice === notice) player.recipeNotice = null;
    }, DISPLAY_MS);
    return () => clearTimeout(timer);
  });
</script>

<!-- The station sheet shows it itself while open. -->
{#if player.recipeNotice && !ui.stationOpen}
  {@const notice = player.recipeNotice}
  <aside class={styles.notice} data-ok={notice.ok} role="status" aria-label="Recipe saved">
    <p class={styles.text}>
      <span class={styles.file}>{notice.file}</span>
      {notice.message}
    </p>
    <button class={styles.close} type="button" aria-label="Dismiss" onclick={() => (player.recipeNotice = null)}
      >✕</button
    >
  </aside>
{/if}
