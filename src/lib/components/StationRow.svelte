<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import Icon from './Icon.svelte';
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
    <Icon name="play" size={14} />
  </button>

  <div class={styles.text}>
    <p class={styles.name} title={name}>
      {#if active}<span class={styles.onAir} aria-hidden="true"></span>{/if}{name}
      {#if personal}<span class={styles.badge}>personal</span>{/if}
    </p>
    {#if meta}<p class={styles.meta} title={meta}>{meta}</p>{/if}
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
      onclick={onremove}><Icon name="x" /></button
    >
  {/if}
  <button
    class={styles.icon}
    type="button"
    aria-pressed={favorite}
    aria-label={favorite ? `Remove ${name} from favorites` : `Add ${name} to favorites`}
    data-on={favorite}
    onclick={onfavorite}><Icon name="star" filled={favorite} /></button
  >
</li>
