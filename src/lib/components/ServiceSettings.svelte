<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Settings › Find this track on: the sites in a track's menu, including the user's own. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { errorMessage } from '../api';
  import { checkCustom, MAX_CUSTOM, SERVICE_GROUPS, TRACK_SERVICES } from '../trackLinks';
  import { trackServices } from '../trackServices.svelte';
  import Icon from './Icon.svelte';
  import styles from './SettingsPage.module.css';

  onMount(() => trackServices.load('settings'));

  // Form to add a site to the track menu.
  let adding = $state(false);
  let siteName = $state('');
  let siteAddress = $state('');
  /** Problems are shown once the user tried to add, then follow the typing. */
  let tried = $state(false);
  let saveError = $state<string | null>(null);
  let saving = $state(false);
  let nameInput: HTMLInputElement | undefined = $state();

  const siteProblem = $derived(checkCustom(siteName, siteAddress));
  const sitePreview = $derived(
    siteProblem
      ? null
      : siteAddress.trim().replace('{query}', encodeURIComponent('Artist - Title')),
  );

  function openForm() {
    adding = true;
    queueMicrotask(() => nameInput?.focus());
  }

  function closeForm() {
    adding = false;
    siteName = siteAddress = '';
    tried = false;
    saveError = null;
  }

  async function addSite(e: SubmitEvent) {
    e.preventDefault();
    tried = true;
    saveError = null;
    if (siteProblem || saving) return;
    saving = true;
    try {
      await trackServices.add(siteName, siteAddress);
      closeForm();
    } catch (err) {
      saveError = errorMessage(err);
    } finally {
      saving = false;
    }
  }
</script>

<section class={styles.group} aria-labelledby="settings-services">
  <h2 class={styles.groupTitle} id="settings-services">Find this track on</h2>
  <p class={styles.groupIntro}>
    Choose the sites listed in the menu of a track in your history. They open a search in your
    browser: no account, nothing sent until you click.
  </p>

  {#each SERVICE_GROUPS as group (group.id)}
    <fieldset class={styles.services}>
      <legend class={styles.servicesLegend}>
        <span class={styles.name}>{group.label}</span>
        <span class={styles.detail}>{group.detail}</span>
      </legend>
      <div class={styles.chips}>
        {#each TRACK_SERVICES.filter((s) => s.group === group.id) as service (service.id)}
          <label class={styles.chip}>
            <input
              type="checkbox"
              checked={trackServices.ids.includes(service.id)}
              onchange={(e) => trackServices.toggle(service.id, e.currentTarget.checked)}
            />
            {service.name}
          </label>
        {/each}
      </div>
    </fieldset>
  {/each}

  <fieldset class={styles.services}>
    <legend class={styles.servicesLegend}>
      <span class={styles.name}>Your sites</span>
      <span class={styles.detail}
        >Any site with a search page, such as your Invidious or PeerTube instance</span
      >
    </legend>
    {#if trackServices.custom.length}
      <div class={styles.chips}>
        {#each trackServices.custom as site (site.id)}
          <span class={styles.customChip}>
            <label class={styles.chip}>
              <input
                type="checkbox"
                checked={trackServices.ids.includes(site.id)}
                onchange={(e) => trackServices.toggle(site.id, e.currentTarget.checked)}
              />
              {site.name}
            </label>
            <button
              class={styles.remove}
              type="button"
              aria-label={`Remove ${site.name}`}
              title={site.template}
              onclick={() => trackServices.remove(site.id)}><Icon name="x" size={14} /></button
            >
          </span>
        {/each}
      </div>
    {/if}

    {#if adding}
      <form class={styles.siteForm} onsubmit={addSite} novalidate>
        <label class={styles.field}>
          <span>Name</span>
          <input
            bind:this={nameInput}
            bind:value={siteName}
            maxlength="40"
            placeholder="Invidious"
            autocomplete="off"
          />
        </label>
        <label class={styles.field}>
          <span>Search address</span>
          <input
            bind:value={siteAddress}
            type="url"
            placeholder={'https://yewtu.be/search?q={query}'}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <p class={styles.hint}>
          Search for any word on the site, copy the address of the results page, and replace your
          word with
          <code>{'{query}'}</code>. Only https:// addresses.
        </p>
        {#if tried && (siteProblem || saveError)}
          <p class={styles.formError} role="alert">{siteProblem ?? saveError}</p>
        {:else if sitePreview}
          <p class={styles.hint}>Opens <code>{sitePreview}</code></p>
        {/if}
        <div class={styles.formActions}>
          <button class={styles.link} type="button" onclick={closeForm}>Cancel</button>
          <button class={styles.primary} type="submit" disabled={saving}>Add</button>
        </div>
      </form>
    {:else if trackServices.custom.length < MAX_CUSTOM}
      <button class={styles.addSite} type="button" onclick={openForm}
        ><Icon name="plus" size={14} /> Add a site</button
      >
    {:else}
      <p class={styles.hint}>You can add up to {MAX_CUSTOM} sites.</p>
    {/if}
  </fieldset>

  <p class={styles.servicesFooter}>
    <span>
      {trackServices.enabled.length === 0
        ? 'The menu shows no site.'
        : `${trackServices.enabled.length} shown in the menu.`}
    </span>
    {#if !trackServices.isDefault}
      <button class={styles.link} type="button" onclick={trackServices.reset}
        >Restore the default list</button
      >
    {/if}
  </p>
</section>
