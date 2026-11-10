<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Settings › Privacy: everything that sends something off this device, off unless allowed. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type LocalRecipes } from '../api';
  import { errors } from '../errors.svelte';
  import { player } from '../player.svelte';
  import { updates } from '../updates.svelte';
  import SettingSwitch from './SettingSwitch.svelte';
  import styles from './SettingsPage.module.css';

  let clickCounting = $state(true);
  let identify = $state(false);
  let webTitles = $state(true);
  /** The sender's name in share links (docs/16-sharing.md §4.2). */
  let shareName = $state('');
  /** New versions: hidden where the app does not check (mobile, nightly, packaged copies). */
  let updateCheck = $state<{ supported: boolean; enabled: boolean }>({
    supported: false,
    enabled: false,
  });
  let localRecipes = $state<LocalRecipes | null>(null);

  onMount(async () => {
    api
      .getShareName()
      .then((name) => (shareName = name))
      .catch((e) => errors.report('settings', e));
    api
      .getUpdateCheck()
      .then((check) => (updateCheck = check))
      .catch((e) => errors.report('settings', e));
    try {
      [clickCounting, identify, webTitles] = await Promise.all([
        api.getClickCounting(),
        api.getTrackIdentification(),
        api.getWebTitles(),
      ]);
    } catch (e) {
      errors.report('settings', e);
    }
  });

  // Loaded with the page, and again after a recipe file is saved.
  $effect(() => {
    void player.recipeNotice;
    api
      .getLocalRecipes()
      .then((r) => (localRecipes = r))
      // eslint-disable-next-line no-restricted-syntax -- shown as no local recipes; a saved recipe reports its own errors
      .catch(() => (localRecipes = null));
  });

  async function saveClickCounting(enabled: boolean) {
    await api.setClickCounting(enabled);
    if (!enabled) player.hideClickNotice();
  }

  async function saveUpdateCheck(enabled: boolean) {
    await api.setUpdateCheck(enabled);
    await updates.load();
  }

  async function openRecipesFolder() {
    try {
      await api.openRecipesFolder();
    } catch (e) {
      errors.report('settings', e);
    }
  }
</script>

<section class={styles.group} aria-labelledby="settings-privacy">
  <h2 class={styles.groupTitle} id="settings-privacy">Privacy</h2>
  <p class={styles.groupIntro}>
    Soundfolk sends nothing about you unless you allow it here. Your history, likes and stations
    stay on this device.
  </p>

  <SettingSwitch
    name="Report my listens to Radio-Browser"
    bind:checked={clickCounting}
    save={saveClickCounting}
  >
    Each station you start is counted (station + IP address) in the open Radio-Browser catalog,
    which helps small radios get discovered.
  </SettingSwitch>

  {#if updateCheck.supported}
    <SettingSwitch
      name="Tell me when a new version is out"
      bind:checked={updateCheck.enabled}
      save={saveUpdateCheck}
    >
      Once a day, Soundfolk asks GitHub for the number of the latest version (GitHub sees your IP
      address). Nothing is installed: you update the way you installed it.
    </SettingSwitch>
  {/if}

  <SettingSwitch
    name="Find the track on air on the station's website"
    bind:checked={webTitles}
    save={api.setWebTitles}
  >
    When a stream doesn't announce its tracks, Soundfolk reads the station's web page or its
    streaming server's status, only while you listen to it. That site sees your IP address. The list
    of recipes that say where to look is downloaded from GitHub once a day. Applies from the next
    station you play.
  </SettingSwitch>

  {#if webTitles}
    <div class={styles.option}>
      <span class={styles.text}>
        <span class={styles.name}>My recipes</span>
        <span class={styles.detail}>
          {#if !localRecipes || localRecipes.count === 0}
            Recipes you write to read a station's website. Start from Station details in the player.
          {:else}
            {localRecipes.count === 1
              ? '1 recipe'
              : `${localRecipes.count} recipes`}{#if localRecipes.errors.length}, {localRecipes
                .errors.length} not used{/if}.
          {/if}
        </span>
        {#each localRecipes?.errors ?? [] as problem (problem.file)}
          <span class={styles.recipeError}>{problem.file}: {problem.error}</span>
        {/each}
      </span>
      <button class="{styles.link} {styles.folder}" type="button" onclick={openRecipesFolder}
        >Open folder</button
      >
    </div>
  {/if}

  <SettingSwitch
    name="Identify my liked tracks on MusicBrainz"
    bind:checked={identify}
    save={api.setTrackIdentification}
  >
    The artist and title of tracks you like (+ IP address) are looked up in the open MusicBrainz
    database to show artist, album, genre, decade and country stats.
  </SettingSwitch>

  <div class={styles.option}>
    <label class={styles.field}>
      <span class={styles.name}>Your name in shared links</span>
      <input
        type="text"
        maxlength="40"
        placeholder="Empty: links don't say who sent them"
        autocomplete="off"
        value={shareName}
        onchange={(e) =>
          api
            .setShareName(e.currentTarget.value)
            .then((kept) => (shareName = kept))
            .catch((err) => errors.report('settings', err))}
      />
      <span class={styles.detail}>
        Every link you share then says it comes from you. Whoever opens it sees the name as you
        typed it.
      </span>
    </label>
  </div>
</section>
