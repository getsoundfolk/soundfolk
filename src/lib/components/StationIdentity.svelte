<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- The station sheet's rows the listener changes: the name the station is shown under and, for a
     catalog station, its logo. Rows of the sheet's details list (`<dl>`). -->
<script lang="ts">
  import { api, errorMessage } from '../api';
  import { stationNames } from '../stationNames.svelte';
  import { isCatalogId } from '../stations';
  import { ui } from '../ui.svelte';
  import styles from './StationSheet.module.css';

  let { station }: { station: { id: string; name: string } } = $props();

  /** The name the user gave the station, if any; the station keeps its own as `station.name`. */
  const given = $derived(stationNames.given(station.id));
  let nameError = $state<string | null>(null);
  /** The logo the user chose for the station (`data:` address), over its catalog favicon. */
  let chosenLogo = $state<string | null>(null);
  let logoError = $state<string | null>(null);
  let logoInput: HTMLInputElement | undefined = $state();

  // The errors belong to a visit: gone when the sheet opens again.
  $effect(() => {
    if (!ui.stationOpen) return;
    nameError = null;
    logoError = null;
  });

  // Only a catalog station can be given a logo (its UUID names the file).
  $effect(() => {
    chosenLogo = null;
    const id = station.id;
    if (!ui.stationOpen || !isCatalogId(id)) return;
    let current = true;
    api
      .getStationLogo(id)
      .then((logo) => {
        if (current) chosenLogo = logo;
      })
      // eslint-disable-next-line no-restricted-syntax -- shown as no image chosen
      .catch(() => {});
    return () => {
      current = false;
    };
  });

  /** Makes an image file the station's logo (null: gives it back its own). */
  async function setLogo(file: File | null) {
    const id = station.id;
    logoError = null;
    try {
      const image = file
        ? await new Promise<string>((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result as string);
            reader.onerror = () => reject(new Error('This file could not be read.'));
            reader.readAsDataURL(file);
          })
        : null;
      const kept = await api.setStationLogo(id, image);
      if (station.id === id) chosenLogo = kept;
    } catch (e) {
      logoError = errorMessage(e);
    }
  }

  /** Saves the name typed (on Enter or when the field loses focus). Its own name, or a blank
   *  field, gives the station back its own name. */
  async function rename(input: HTMLInputElement) {
    nameError = null;
    try {
      const typed = input.value.trim();
      await stationNames.rename(station.id, typed === station.name.trim() ? '' : typed);
    } catch (e) {
      nameError = errorMessage(e);
    }
    input.value = stationNames.given(station.id) ?? station.name;
  }
</script>

<div>
  <dt><label for="station-sheet-name">Name</label></dt>
  <dd>
    <input
      id="station-sheet-name"
      class={styles.input}
      type="text"
      value={given ?? station.name}
      aria-describedby="station-sheet-name-hint"
      spellcheck="false"
      autocomplete="off"
      onkeydown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur();
        // Escape closes the sheet: the name typed is dropped, not saved.
        else if (e.key === 'Escape') e.currentTarget.value = given ?? station.name;
      }}
      onchange={(e) => rename(e.currentTarget)}
    />
    <p class={styles.hint} id="station-sheet-name-hint">
      {#if given}
        Shown instead of its own name, {station.name}.
        <button
          class={styles.inline}
          type="button"
          onclick={() =>
            stationNames.rename(station.id, '').catch((e) => (nameError = errorMessage(e)))}
          >Use its own name</button
        >
      {:else}
        Change it to see this station under another name.
      {/if}
    </p>
    {#if nameError}<p class={styles.warning} role="alert">{nameError}</p>{/if}
  </dd>
</div>
{#if isCatalogId(station.id)}
  <div>
    <dt>Logo</dt>
    <dd>
      <div class={styles.logoRow}>
        {#if chosenLogo}<img class={styles.logo} src={chosenLogo} alt="" />{/if}
        <button class={styles.chip} type="button" onclick={() => logoInput?.click()}
          >{chosenLogo ? 'Choose another image…' : 'Choose an image…'}</button
        >
        <input
          bind:this={logoInput}
          type="file"
          accept="image/png,image/jpeg,image/gif,image/webp,image/bmp,image/svg+xml,.ico"
          hidden
          onchange={(e) => {
            const file = e.currentTarget.files?.[0] ?? null;
            e.currentTarget.value = '';
            if (file) setLogo(file);
          }}
        />
      </div>
      <p class={styles.hint}>
        {#if chosenLogo}
          Shown instead of its own logo.
          <button class={styles.inline} type="button" onclick={() => setLogo(null)}
            >Use its own logo</button
          >
        {:else}
          An image file of yours, shown instead of its own logo.
        {/if}
      </p>
      {#if logoError}<p class={styles.warning} role="alert">{logoError}</p>{/if}
    </dd>
  </div>
{/if}
