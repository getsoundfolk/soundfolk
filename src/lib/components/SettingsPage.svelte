<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { onMount } from 'svelte';
  import { api, errorMessage, type LocalRecipes } from '../api';
  import { player } from '../player.svelte';
  import { ui } from '../ui.svelte';
  import styles from './SettingsPage.module.css';

  let clickCounting = $state(true);
  let identify = $state(false);
  let webTitles = $state(true);
  let version = $state<string | null>(null);
  let back: HTMLButtonElement | undefined = $state();

  onMount(async () => {
    back?.focus();
    try {
      [clickCounting, identify, webTitles] = await Promise.all([
        api.getClickCounting(),
        api.getTrackIdentification(),
        api.getWebTitles(),
      ]);
    } catch (e) {
      player.error = errorMessage(e);
    }
    version = await getVersion().catch(() => null);
  });

  async function changeClickCounting(enabled: boolean) {
    clickCounting = enabled;
    try {
      await api.setClickCounting(enabled);
      if (!enabled) player.clickNotice = false;
    } catch (e) {
      clickCounting = !enabled;
      player.error = errorMessage(e);
    }
  }

  async function changeWebTitles(enabled: boolean) {
    webTitles = enabled;
    try {
      await api.setWebTitles(enabled);
    } catch (e) {
      webTitles = !enabled;
      player.error = errorMessage(e);
    }
  }

  let localRecipes = $state<LocalRecipes | null>(null);

  // Loaded with the page, and again after a recipe file is saved.
  $effect(() => {
    void player.recipeNotice;
    api
      .getLocalRecipes()
      .then((r) => (localRecipes = r))
      .catch(() => (localRecipes = null));
  });

  async function openRecipesFolder() {
    try {
      await api.openRecipesFolder();
    } catch (e) {
      player.error = errorMessage(e);
    }
  }

  async function changeIdentify(enabled: boolean) {
    identify = enabled;
    try {
      await api.setTrackIdentification(enabled);
    } catch (e) {
      identify = !enabled;
      player.error = errorMessage(e);
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && ui.closeSettings()} />

<section class={styles.root} aria-labelledby="settings-title">
  <header class={styles.header}>
    <button class={styles.back} type="button" bind:this={back} onclick={ui.closeSettings}>
      <span aria-hidden="true">←</span> Back
    </button>
    <h1 class={styles.heading} id="settings-title">Settings</h1>
  </header>

  {#if player.error}<p class={styles.error} role="alert">{player.error}</p>{/if}

  <div class={styles.body}>
    <section class={styles.group} aria-labelledby="settings-privacy">
      <h2 class={styles.groupTitle} id="settings-privacy">Privacy</h2>
      <p class={styles.groupIntro}>
        Soundfolk sends nothing about you unless you allow it here. Your history, likes and stations stay on this
        device.
      </p>

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Report my listens to Radio-Browser</span>
          <span class={styles.detail}>
            Each station you start is counted (station + IP address) in the open Radio-Browser catalog, which helps
            small radios get discovered.
          </span>
        </span>
        <input
          class={styles.toggle}
          type="checkbox"
          role="switch"
          checked={clickCounting}
          onchange={(e) => changeClickCounting(e.currentTarget.checked)}
        />
      </label>

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Find the track on air on the station's website</span>
          <span class={styles.detail}>
            When a stream doesn't announce its tracks, Soundfolk reads the station's web page or its streaming
            server's status, only while you listen to it. That site sees your IP address. Applies from the next
            station you play.
          </span>
        </span>
        <input
          class={styles.toggle}
          type="checkbox"
          role="switch"
          checked={webTitles}
          onchange={(e) => changeWebTitles(e.currentTarget.checked)}
        />
      </label>

      {#if webTitles}
        <div class={styles.option}>
          <span class={styles.text}>
            <span class={styles.name}>My recipes</span>
            <span class={styles.detail}>
              {#if !localRecipes || localRecipes.count === 0}
                Recipes you write to read a station's website. Start from a station's details (ⓘ in the player).
              {:else}
                {localRecipes.count === 1 ? '1 recipe' : `${localRecipes.count} recipes`}{#if localRecipes.errors.length}, {localRecipes.errors.length} not used{/if}.
              {/if}
            </span>
            {#each localRecipes?.errors ?? [] as problem (problem.file)}
              <span class={styles.recipeError}>{problem.file}: {problem.error}</span>
            {/each}
          </span>
          <button class="{styles.link} {styles.folder}" type="button" onclick={openRecipesFolder}>Open folder</button>
        </div>
      {/if}

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Identify my liked tracks on MusicBrainz</span>
          <span class={styles.detail}>
            The artist and title of tracks you like (+ IP address) are looked up in the open MusicBrainz database to
            show artist, album, genre, decade and country stats.
          </span>
        </span>
        <input
          class={styles.toggle}
          type="checkbox"
          role="switch"
          checked={identify}
          onchange={(e) => changeIdentify(e.currentTarget.checked)}
        />
      </label>
    </section>

    <section class={styles.group} aria-labelledby="settings-about">
      <h2 class={styles.groupTitle} id="settings-about">About</h2>
      <dl class={styles.about}>
        <div>
          <dt>Version</dt>
          <dd>{version ?? '—'}</dd>
        </div>
        <div>
          <dt>License</dt>
          <dd>MPL-2.0</dd>
        </div>
        <div>
          <dt>Station catalog</dt>
          <dd>Radio-Browser</dd>
        </div>
        <div>
          <dt>Now-playing recipes</dt>
          <dd>soundfolk-recipes contributors (ODbL)</dd>
        </div>
      </dl>
    </section>
  </div>
</section>
