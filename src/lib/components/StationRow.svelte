<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import styles from './StationRow.module.css';

  interface Props {
    name: string;
    meta?: string;
    /** `null` = healthy station; otherwise the warning to display. */
    warning?: string | null;
    personal?: boolean;
    active?: boolean;
    favorite: boolean;
    onplay: () => void;
    onfavorite: () => void;
    onremove?: () => void;
  }

  let {
    name,
    meta = '',
    warning = null,
    personal = false,
    active = false,
    favorite,
    onplay,
    onfavorite,
    onremove,
  }: Props = $props();
</script>

<li class={styles.row} data-active={active}>
  <button
    class={styles.play}
    type="button"
    title={`Play ${name}`}
    aria-label={`Play ${name}`}
    onclick={onplay}
  >
    ▶
  </button>

  <div class={styles.text}>
    <p class={styles.name}>
      {name}
      {#if personal}<span class={styles.badge}>personal</span>{/if}
    </p>
    {#if meta}<p class={styles.meta}>{meta}</p>{/if}
    {#if warning}
      <p class={styles.warning}>{warning}</p>
    {/if}
  </div>

  {#if onremove}
    <button
      class={styles.icon}
      type="button"
      aria-label={`Remove ${name}`}
      title="Remove"
      onclick={onremove}>✕</button
    >
  {/if}
  <button
    class={styles.icon}
    type="button"
    aria-pressed={favorite}
    aria-label={favorite ? `Remove ${name} from favorites` : `Add ${name} to favorites`}
    data-on={favorite}
    onclick={onfavorite}>{favorite ? '★' : '☆'}</button
  >
</li>
