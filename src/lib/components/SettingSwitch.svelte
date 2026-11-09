<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- One on/off setting of the settings page: its name, what it does, and the switch. -->
<script module lang="ts">
  import { errors } from '../errors.svelte';

  /** Shows the new value at once and saves it; when the save fails, the old value comes back
   *  and the error is shown on the settings page. */
  export async function toggleSetting(
    enabled: boolean,
    set: (enabled: boolean) => void,
    save: (enabled: boolean) => Promise<unknown>,
  ) {
    set(enabled);
    try {
      await save(enabled);
    } catch (e) {
      set(!enabled);
      errors.report('settings', e);
    }
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';
  import styles from './SettingsPage.module.css';

  let {
    name,
    checked = $bindable(),
    save,
    children,
  }: {
    name: string;
    checked: boolean;
    /** Saves the new value; throws when it was not saved. */
    save: (enabled: boolean) => Promise<unknown>;
    /** What the setting does. */
    children: Snippet;
  } = $props();
</script>

<label class={styles.option}>
  <span class={styles.text}>
    <span class={styles.name}>{name}</span>
    <span class={styles.detail}>{@render children()}</span>
  </span>
  <input
    class={styles.toggle}
    type="checkbox"
    role="switch"
    {checked}
    onchange={(e) => toggleSetting(e.currentTarget.checked, (on) => (checked = on), save)}
  />
</label>
