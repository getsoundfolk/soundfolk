<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { onMount } from 'svelte';
  import {
    api,
    errorMessage,
    type CoverSource,
    type CoverSourceId,
    type HistorySummary,
    type LocalRecipes,
  } from '../api';
  import { rovingKeydown } from '../a11y';
  import { errors } from '../errors.svelte';
  import { history } from '../history.svelte';
  import { player } from '../player.svelte';
  import { checkCustom, MAX_CUSTOM, SERVICE_GROUPS, TRACK_SERVICES } from '../trackLinks';
  import { savedTheme, setTheme, type Theme } from '../theme';
  import { trackServices } from '../trackServices.svelte';
  import { ui } from '../ui.svelte';
  import { updates } from '../updates.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import OrderedSources, { type SourceItem } from './OrderedSources.svelte';
  import Icon from './Icon.svelte';
  import styles from './SettingsPage.module.css';

  let clickCounting = $state(true);
  let identify = $state(false);
  let webTitles = $state(true);
  let autostart = $state(false);
  let trackNotifications = $state(false);
  let constantVolume = $state(true);
  /** The sender's name in share links (docs/16-sharing.md §4.2). */
  let shareName = $state('');
  let version = $state<string | null>(null);
  /** New versions: hidden where the app does not check (mobile, nightly, packaged copies). */
  let updateCheck = $state<{ supported: boolean; enabled: boolean }>({
    supported: false,
    enabled: false,
  });
  let back: HTMLButtonElement | undefined = $state();
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
    // An error left from an earlier visit is stale.
    errors.clear('settings');
    trackServices.load('settings');
    loadSummary();
    api
      .getShareName()
      .then((name) => (shareName = name))
      .catch((e) => errors.report('settings', e));
    api
      .getCoverSources()
      .then((s) => (coverSources = s))
      .catch((e) => errors.report('settings', e));
    api
      .getUpdateCheck()
      .then((check) => (updateCheck = check))
      .catch(() => {});
    api
      .getAutostart()
      .then((enabled) => (autostart = enabled))
      .catch((e) => errors.report('settings', e));
    try {
      [clickCounting, identify, webTitles, trackNotifications, constantVolume] = await Promise.all([
        api.getClickCounting(),
        api.getTrackIdentification(),
        api.getWebTitles(),
        api.getTrackNotifications(),
        api.getConstantVolume(),
      ]);
    } catch (e) {
      errors.report('settings', e);
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
      errors.report('settings', e);
    }
  }

  async function changeUpdateCheck(enabled: boolean) {
    updateCheck.enabled = enabled;
    try {
      await api.setUpdateCheck(enabled);
      await updates.load();
    } catch (e) {
      updateCheck.enabled = !enabled;
      errors.report('settings', e);
    }
  }

  async function skipUpdate() {
    try {
      await updates.skip();
    } catch (e) {
      errors.report('settings', e);
    }
  }

  async function changeAutostart(enabled: boolean) {
    autostart = enabled;
    try {
      await api.setAutostart(enabled);
    } catch (e) {
      autostart = !enabled;
      errors.report('settings', e);
    }
  }

  async function changeTrackNotifications(enabled: boolean) {
    trackNotifications = enabled;
    try {
      await api.setTrackNotifications(enabled);
    } catch (e) {
      trackNotifications = !enabled;
      errors.report('settings', e);
    }
  }

  async function changeConstantVolume(enabled: boolean) {
    constantVolume = enabled;
    try {
      await api.setConstantVolume(enabled);
    } catch (e) {
      constantVolume = !enabled;
      errors.report('settings', e);
    }
  }

  async function changeWebTitles(enabled: boolean) {
    webTitles = enabled;
    try {
      await api.setWebTitles(enabled);
    } catch (e) {
      webTitles = !enabled;
      errors.report('settings', e);
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
      errors.report('settings', e);
    }
  }

  // ── Your data ──
  let summary = $state<HistorySummary | null>(null);
  let exporting = $state(false);
  let confirmingClear = $state(false);
  let clearing = $state(false);
  let dataStatus = $state<string | null>(null);
  let cancelClear: HTMLButtonElement | undefined = $state();

  const tracks = (n: number) => `${n.toLocaleString()} ${n === 1 ? 'track' : 'tracks'}`;
  const since = $derived(
    summary?.since
      ? new Date(`${summary.since.replace(' ', 'T')}Z`).toLocaleDateString(undefined, {
          dateStyle: 'long',
        })
      : null,
  );

  async function loadSummary() {
    try {
      summary = await api.getHistorySummary();
    } catch (e) {
      errors.report('settings', e);
    }
  }

  async function exportHistory(format: 'csv' | 'json') {
    if (exporting) return;
    exporting = true;
    dataStatus = null;
    try {
      const file = await api.exportHistory('history', format, null);
      dataStatus = `${tracks(file.count)} saved to Downloads: ${file.file_name}`;
    } catch (e) {
      errors.report('settings', e);
    } finally {
      exporting = false;
    }
  }

  function askClear() {
    confirmingClear = true;
    dataStatus = null;
    // The safe answer has the focus: Enter does not delete.
    queueMicrotask(() => cancelClear?.focus());
  }

  async function clearHistory() {
    if (clearing) return;
    clearing = true;
    try {
      const removed = await api.clearListeningHistory();
      history.latest = null;
      history.version++;
      dataStatus = `${tracks(removed)} deleted.`;
      confirmingClear = false;
      await loadSummary();
    } catch (e) {
      errors.report('settings', e);
    } finally {
      clearing = false;
    }
  }

  async function changeIdentify(enabled: boolean) {
    identify = enabled;
    try {
      await api.setTrackIdentification(enabled);
    } catch (e) {
      identify = !enabled;
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

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Launch at login</span>
          <span class={styles.detail}>
            Soundfolk starts in the notification area when you log in, without opening its window or
            playing.
          </span>
        </span>
        <input
          class={styles.toggle}
          type="checkbox"
          role="switch"
          checked={autostart}
          onchange={(e) => changeAutostart(e.currentTarget.checked)}
        />
      </label>
    </section>

    <section class={styles.group} aria-labelledby="settings-playback">
      <h2 class={styles.groupTitle} id="settings-playback">Playback</h2>

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Constant volume</span>
          <span class={styles.detail}>Keeps the same level from one station to the next.</span>
        </span>
        <input
          class={styles.toggle}
          type="checkbox"
          role="switch"
          checked={constantVolume}
          onchange={(e) => changeConstantVolume(e.currentTarget.checked)}
        />
      </label>
    </section>

    <section class={styles.group} aria-labelledby="settings-notifications">
      <h2 class={styles.groupTitle} id="settings-notifications">Notifications</h2>

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Track notifications</span>
          <span class={styles.detail}>
            Shows a notification when a new track starts and the Soundfolk window is in the
            background.
          </span>
        </span>
        <input
          class={styles.toggle}
          type="checkbox"
          role="switch"
          checked={trackNotifications}
          onchange={(e) => changeTrackNotifications(e.currentTarget.checked)}
        />
      </label>
    </section>

    <section class={styles.group} aria-labelledby="settings-privacy">
      <h2 class={styles.groupTitle} id="settings-privacy">Privacy</h2>
      <p class={styles.groupIntro}>
        Soundfolk sends nothing about you unless you allow it here. Your history, likes and stations
        stay on this device.
      </p>

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Report my listens to Radio-Browser</span>
          <span class={styles.detail}>
            Each station you start is counted (station + IP address) in the open Radio-Browser
            catalog, which helps small radios get discovered.
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

      {#if updateCheck.supported}
        <label class={styles.option}>
          <span class={styles.text}>
            <span class={styles.name}>Tell me when a new version is out</span>
            <span class={styles.detail}>
              Once a day, Soundfolk asks GitHub for the number of the latest version (GitHub sees
              your IP address). Nothing is installed: you update the way you installed it.
            </span>
          </span>
          <input
            class={styles.toggle}
            type="checkbox"
            role="switch"
            checked={updateCheck.enabled}
            onchange={(e) => changeUpdateCheck(e.currentTarget.checked)}
          />
        </label>
      {/if}

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Find the track on air on the station's website</span>
          <span class={styles.detail}>
            When a stream doesn't announce its tracks, Soundfolk reads the station's web page or its
            streaming server's status, only while you listen to it. That site sees your IP address.
            The list of recipes that say where to look is downloaded from GitHub once a day. Applies
            from the next station you play.
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
                Recipes you write to read a station's website. Start from Station details in the
                player.
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

      <label class={styles.option}>
        <span class={styles.text}>
          <span class={styles.name}>Identify my liked tracks on MusicBrainz</span>
          <span class={styles.detail}>
            The artist and title of tracks you like (+ IP address) are looked up in the open
            MusicBrainz database to show artist, album, genre, decade and country stats.
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
              Search for any word on the site, copy the address of the results page, and replace
              your word with
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

    <section class={styles.group} aria-labelledby="settings-data">
      <h2 class={styles.groupTitle} id="settings-data">Your data</h2>
      <p class={styles.groupIntro}>
        Your listening history is kept on this device only. It feeds the History tab and the stats.
      </p>

      <div class={styles.dataRow}>
        <span class={styles.text}>
          <span class={styles.name}>Export my history</span>
          <span class={styles.detail}>
            Every track heard, with its station and time, as a file in your Downloads folder. Liked
            tracks alone can be exported from History › Liked.
          </span>
        </span>
        <span class={styles.dataActions}>
          <button
            class={styles.link}
            type="button"
            disabled={exporting || !summary?.count}
            onclick={() => exportHistory('csv')}>CSV</button
          >
          <button
            class={styles.link}
            type="button"
            disabled={exporting || !summary?.count}
            onclick={() => exportHistory('json')}>JSON</button
          >
        </span>
      </div>

      <div class={styles.dataRow} data-danger={confirmingClear}>
        <span class={styles.text}>
          <span class={styles.name}>Clear my history</span>
          <span class={styles.detail}>
            {#if !summary}
              …
            {:else if summary.count === 0}
              The history is empty.
            {:else}
              {tracks(summary.count)}{since ? ` since ${since}` : ''}. Liked tracks are kept.
            {/if}
          </span>
          {#if confirmingClear && summary?.count}
            <span class={styles.warning} role="alert">
              This deletes for good: {tracks(summary.count)}, the listening time of every station
              (all the stats start over), and the volume Constant volume learned for each station.
              Liked tracks are kept. This cannot be undone: export first if you may want the tracks
              back.
            </span>
            <span class={styles.formActions}>
              <button
                class={styles.link}
                type="button"
                bind:this={cancelClear}
                onclick={() => (confirmingClear = false)}>Cancel</button
              >
              <button
                class={styles.danger}
                type="button"
                disabled={clearing}
                onclick={clearHistory}
              >
                Delete {tracks(summary.count)}
              </button>
            </span>
          {/if}
        </span>
        {#if !confirmingClear}
          <button
            class="{styles.link} {styles.dataActions}"
            type="button"
            disabled={!summary?.count}
            onclick={askClear}>Clear…</button
          >
        {/if}
      </div>

      {#if dataStatus}<p class={styles.hint} role="status">{dataStatus}</p>{/if}
    </section>

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
