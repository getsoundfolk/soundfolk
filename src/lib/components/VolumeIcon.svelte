<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- The speaker of the volume control: crossed out when silent, one wave up to half volume, two
     beyond. -->
<script lang="ts">
  import { player } from '../player.svelte';
  import Icon from './Icon.svelte';

  let { size = 16 }: { size?: number } = $props();

  const level = $derived(player.muted ? 0 : player.volume);
</script>

<Icon name={level === 0 ? 'volume-off' : level > 0.5 ? 'volume' : 'volume-low'} {size} />
