<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Previous / next favorite, as in the tray and the media keys: it changes the station, not the
     track (a live stream has no previous track). -->
<script lang="ts">
  import { api } from '../api';
  import { errors } from '../errors.svelte';
  import { player } from '../player.svelte';
  import Icon from './Icon.svelte';

  let {
    forward,
    class: className,
    size = 16,
  }: { forward: boolean; class: string; size?: number } = $props();

  const label = $derived(forward ? 'Next favorite' : 'Previous favorite');
</script>

<button
  class={className}
  type="button"
  aria-label={label}
  title={label}
  disabled={player.favorites.length === 0}
  onclick={() => api.switchFavorite(forward).catch((e) => errors.report('player', e))}
  ><Icon name={forward ? 'next' : 'previous'} {size} /></button
>
