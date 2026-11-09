<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- ♥ for the track on air, once the history has logged it; nothing before. A failure shows in
     the player. -->
<script lang="ts">
  import { history } from '../history.svelte';
  import Icon from './Icon.svelte';

  let { class: className, size = 16 }: { class: string; size?: number } = $props();
</script>

{#if history.onAir}
  {@const track = history.onAir}
  <button
    class={className}
    type="button"
    aria-pressed={track.liked}
    aria-label={track.liked ? `Unlike ${track.title}` : `Like ${track.title}`}
    title={track.liked ? 'Unlike' : 'Like'}
    data-on={track.liked}
    onclick={() => history.toggleLike({ ...track, historyId: track.id }, 'player')}
    ><Icon name="heart" {size} filled={track.liked} /></button
  >
{/if}
