<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { api, errorMessage, type PersonalStation } from '../api';
  import styles from './AddStationForm.module.css';

  let { onadded }: { onadded: (station: PersonalStation) => void } = $props();

  let url = $state('');
  let name = $state('');
  let pending = $state(false);
  let error = $state<string | null>(null);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    pending = true;
    error = null;
    try {
      const station = await api.addPersonalStation(url.trim(), name.trim());
      url = '';
      name = '';
      onadded(station);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
</script>

<form class={styles.form} onsubmit={submit}>
  <input
    class={styles.input}
    type="url"
    required
    placeholder="Stream URL, .m3u or .pls"
    aria-label="Stream URL"
    bind:value={url}
    disabled={pending}
    spellcheck="false"
    autocomplete="off"
  />
  <input
    class={styles.input}
    type="text"
    required
    placeholder="Name"
    aria-label="Station name"
    bind:value={name}
    disabled={pending}
  />
  <button
    class={styles.button}
    type="submit"
    disabled={pending || url.trim() === '' || name.trim() === ''}
    aria-busy={pending}
    title={pending ? 'Testing stream…' : undefined}>Add</button
  >
  <p class={styles.hint}>
    The stream is tested before it is saved. The station stays on this device.
  </p>
  {#if error}<p class={styles.error} role="alert">{error}</p>{/if}
</form>
