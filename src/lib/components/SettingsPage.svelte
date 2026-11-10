<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { onMount } from 'svelte';
  import { api, type CoverSource, type CoverSourceId } from '../api';
  import { rovingKeydown } from '../a11y';
  import { errors } from '../errors.svelte';
  import { savedTheme, setTheme, type Theme } from '../theme';
  import { ui } from '../ui.svelte';
  import { updates } from '../updates.svelte';
  import DataSettings from './DataSettings.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import OrderedSources, { type SourceItem } from './OrderedSources.svelte';
  import Icon from './Icon.svelte';
  import PrivacySettings from './PrivacySettings.svelte';
  import ServiceSettings from './ServiceSettings.svelte';
  import SettingSwitch from './SettingSwitch.svelte';
  import styles from './SettingsPage.module.css';

  // An error left from an earlier visit is stale. Cleared before the sections mount and load.
  errors.clear('settings');

  let autostart = $state(false);
  let trackNotifications = $state(false);
  let constantVolume = $state(true);
  let version = $state<string | null>(null);
  let back: HTMLButtonElement | undefined = $state();
  /** The history deletion waits for its confirmation (Your data). */
  let confirmingClear = $state(false);
  let theme = $state<Theme>(savedTheme());
  const THEMES: { id: Theme; label: string }[] = [
    { id: 'system', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ];

  function changeTheme(next: Theme) {
    theme = next;
    setTheme(next);
  }

  // ── Cover art ──
  const COVER_SOURCES: Record<CoverSourceId, Omit<SourceItem, 'id' | 'enabled'>> = {
    station: {
      name: 'Station',
      badge: 'Nothing sent',
      detail:
        "The cover the station's website or streaming server gives with the title. Few stations do. Needs “Find the track on air on the station's website” above.",
    },
    deezer: {
      name: 'Deezer',
      badge: 'Sends artist & title',
      detail:
        "The artist and title of each track on air (+ IP address) are looked up on Deezer's public search, and its album's cover is loaded from Deezer. Fast; best on well-known music. Once per track.",
    },
    itunes: {
      name: 'iTunes',
      badge: 'Sends artist & title',
      detail:
        "The artist and title of each track on air (+ IP address) are looked up on Apple's iTunes Search, and its album's artwork is loaded from Apple. Fast; best on well-known music. Once per track.",
    },
    cover_art_archive: {
      name: 'Cover Art Archive',
      badge: 'Sends artist & title',
      detail:
        'The artist and title of each track on air (+ IP address) are looked up on MusicBrainz, then the front cover of its album is loaded from the Cover Art Archive (Internet Archive). Covers more independent and older music, but often takes several seconds. Once per track.',
    },
  };
  const DEFAULT_COVER_SOURCES: CoverSource[] = [
    { id: 'station', enabled: true },
    { id: 'deezer', enabled: false },
    { id: 'itunes', enabled: false },
    { id: 'cover_art_archive', enabled: false },
  ];
  let coverSources = $state<CoverSource[]>(DEFAULT_COVER_SOURCES);
  const coverItems = $derived(coverSources.map((s) => ({ ...COVER_SOURCES[s.id], ...s })));
  const coverDefault = $derived(
    coverSources.every(
      (s, i) =>
        s.id === DEFAULT_COVER_SOURCES[i].id && s.enabled === DEFAULT_COVER_SOURCES[i].enabled,
    ),
  );

  async function saveCoverSources(sources: CoverSource[]) {
    const before = coverSources;
    coverSources = sources;
    try {
      coverSources = await api.setCoverSources(sources);
    } catch (e) {
      coverSources = before;
      errors.report('settings', e);
    }
  }

  onMount(async () => {
    back?.focus();
    api
      .getCoverSources()
      .then((s) => (coverSources = s))
      .catch((e) => errors.report('settings', e));
    try {
      [autostart, trackNotifications, constantVolume] = await Promise.all([
        api.getAutostart(),
        api.getTrackNotifications(),
        api.getConstantVolume(),
      ]);
    } catch (e) {
      errors.report('settings', e);
    }
    // eslint-disable-next-line no-restricted-syntax -- the version is shown as —
    version = await getVersion().catch(() => null);
  });

  async function skipUpdate() {
    try {
      await updates.skip();
    } catch (e) {
      errors.report('settings', e);
    }
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key !== 'Escape') return;
    // Escape first cancels a pending deletion, then leaves the page.
    if (confirmingClear) confirmingClear = false;
    else ui.closeSettings();
  }}
/>

<section class={styles.root} aria-labelledby="settings-title">
  <header class={styles.header}>
    <button class={styles.back} type="button" bind:this={back} onclick={ui.closeSettings}>
      <Icon name="back" /> Back
    </button>
    <h1 class={styles.heading} id="settings-title">Settings</h1>
  </header>

  <div class={styles.error}><ErrorBanner area="settings" /></div>

  <div class={styles.body}>
    <section class={styles.group} aria-labelledby="settings-appearance">
      <h2 class={styles.groupTitle} id="settings-appearance">Appearance</h2>

      <div class={styles.option} data-static="true">
        <span class={styles.text}>
          <span class={styles.name} id="settings-theme">Theme</span>
          <span class={styles.detail}
            >System follows the light or dark setting of your desktop.</span
          >
        </span>
        <div
          class={styles.segmented}
          role="radiogroup"
          tabindex="-1"
          aria-labelledby="settings-theme"
          onkeydown={rovingKeydown}
        >
          {#each THEMES as t (t.id)}
            <button
              class={styles.segment}
              type="button"
              role="radio"
              aria-checked={theme === t.id}
              tabindex={theme === t.id ? 0 : -1}
              onclick={() => changeTheme(t.id)}>{t.label}</button
            >
          {/each}
        </div>
      </div>
    </section>

    <section class={styles.group} aria-labelledby="settings-startup">
      <h2 class={styles.groupTitle} id="settings-startup">Startup</h2>

      <SettingSwitch name="Launch at login" bind:checked={autostart} save={api.setAutostart}>
        Soundfolk starts in the notification area when you log in, without opening its window or
        playing.
      </SettingSwitch>
    </section>

    <section class={styles.group} aria-labelledby="settings-playback">
      <h2 class={styles.groupTitle} id="settings-playback">Playback</h2>

      <SettingSwitch
        name="Constant volume"
        bind:checked={constantVolume}
        save={api.setConstantVolume}
      >
        Keeps the same level from one station to the next.
      </SettingSwitch>
    </section>

    <section class={styles.group} aria-labelledby="settings-notifications">
      <h2 class={styles.groupTitle} id="settings-notifications">Notifications</h2>

      <SettingSwitch
        name="Track notifications"
        bind:checked={trackNotifications}
        save={api.setTrackNotifications}
      >
        Shows a notification when a new track starts and the Soundfolk window is in the background.
      </SettingSwitch>
    </section>

    <PrivacySettings />

    <section class={styles.group} aria-labelledby="settings-covers">
      <h2 class={styles.groupTitle} id="settings-covers">Cover art</h2>
      <OrderedSources
        title="Cover art sources"
        intro="Asked from top to bottom: the first cover found is shown in the player. A disabled source is never asked. Without a cover, the station's logo is shown. The desktop's media controls always show the station's logo."
        items={coverItems}
        isDefault={coverDefault}
        onchange={(items) =>
          saveCoverSources(items.map(({ id, enabled }) => ({ id: id as CoverSourceId, enabled })))}
        onreset={() => saveCoverSources(DEFAULT_COVER_SOURCES)}
      />
    </section>

    <ServiceSettings />

    <DataSettings bind:confirmingClear />

    <section class={styles.group} aria-labelledby="settings-about">
      <h2 class={styles.groupTitle} id="settings-about">About</h2>
      <dl class={styles.about}>
        <div>
          <dt>Version</dt>
          <dd>{version ?? '—'}</dd>
        </div>
        {#if updates.available}
          <div class={styles.update}>
            <dt>New version</dt>
            <dd>
              {updates.available.version}
              <button
                class={styles.link}
                type="button"
                onclick={() => updates.open().catch((e) => errors.report('settings', e))}
                >What's new</button
              >
              <button class={styles.link} type="button" onclick={skipUpdate}>Skip</button>
            </dd>
          </div>
        {/if}
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
