<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- The error of one area of the window, until dismissed or cleared by that area's next action. -->
<script lang="ts">
  import { errors, type ErrorArea } from '../errors.svelte';
  import styles from './ErrorBanner.module.css';

  let { area }: { area: ErrorArea } = $props();
</script>

{#if errors[area]}
  <p class={styles.error} role="alert">
    <span class={styles.text}>{errors[area]}</span>
    <button
      class={styles.close}
      type="button"
      aria-label="Dismiss"
      onclick={() => errors.clear(area)}>✕</button
    >
  </p>
{/if}
