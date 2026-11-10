<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { api, type PersonalStation } from '../api';
  import { rovingKeydown } from '../a11y';
  import { errors } from '../errors.svelte';
  import { lists } from '../lists.svelte';
  import { player } from '../player.svelte';
  import { ui } from '../ui.svelte';
  import DiscoverPanel from './DiscoverPanel.svelte';
  import HistoryPanel from './HistoryPanel.svelte';
  import ListenPanel from './ListenPanel.svelte';
  import ListScreen from './ListScreen.svelte';
  import Icon from './Icon.svelte';
  import styles from './StationBrowser.module.css';
  import { updates } from '../updates.svelte';

  // The three destinations of docs/06-ui-principles.md §1: what is mine, what others offer, what I
  // heard.
  type Tab = 'listen' | 'discover' | 'history';
  const TABS: { id: Tab; label: string }[] = [
    { id: 'listen', label: 'Listen' },
    { id: 'discover', label: 'Discover' },
    { id: 'history', label: 'History' },
  ];

  let tab = $state<Tab>('listen');

  // A list opens in the tab it was opened from: a favorite list in Listen, a Discover row in
  // Discover. From History (the station sheet's "In N lists"), it opens in Discover.
  let listTab = $state<Tab>('discover');
  $effect(() => {
    if (!ui.listRequest) return;
    untrack(() => {
      listTab = tab === 'listen' ? 'listen' : 'discover';
      tab = listTab;
    });
  });
  const openedList = $derived(ui.listId ? lists.get(ui.listId) : null);

  // Ctrl+F / Ctrl+K (`shortcuts.ts`): the search lives in Discover.
  let searchField = $state<HTMLInputElement>();
  $effect(() => {
    if (!ui.searchRequest) return;
    untrack(() => (tab = 'discover'));
    tick().then(() => searchField?.select());
  });

  // Personal stations, shown in Listen and searched in Discover.
  let personal = $state<PersonalStation[]>([]);
  const loadPersonal = async () => {
    try {
      personal = await api.listPersonalStations();
    } catch (e) {
      errors.report('stations', e);
    }
  };
  onMount(loadPersonal);
  // Added from a share link (shared sheet): My stations reads them again.
  $effect(() => {
    if (ui.personalChanged) loadPersonal();
  });

  // Back from the settings: focus returns to the gear that opened them.
  let gear: HTMLButtonElement | undefined = $state();
  let wasOpen = false;
  $effect(() => {
    const open = ui.settingsOpen;
    if (wasOpen && !open) gear?.focus();
    wasOpen = open;
  });

  const favoriteCount = $derived(player.favorites.length + lists.favorites.length);
</script>

<section class={styles.root} aria-label="Stations">
  <!-- The tabs, then the window's own buttons at the end of the bar: they act on the app, not on
       the station on air, so they stay out of the player. -->
  <div class={styles.bar}>
    <div class={styles.tabs} role="tablist" tabindex="-1" onkeydown={rovingKeydown}>
      {#each TABS as t (t.id)}
        <button
          class={styles.tab}
          type="button"
          role="tab"
          id={`tab-${t.id}`}
          aria-selected={tab === t.id}
          tabindex={tab === t.id ? 0 : -1}
          aria-controls={tab === t.id ? `panel-${t.id}` : undefined}
          onclick={() => (tab = t.id)}
        >
          {t.label}
          {#if t.id === 'listen' && favoriteCount}<span class={styles.count}>{favoriteCount}</span
            >{/if}
        </button>
      {/each}
    </div>
    <button
      class={styles.tool}
      type="button"
      aria-label="Mini player"
      title="Mini player"
      onclick={() => api.openMiniPlayer().catch((e) => errors.report('player', e))}
      ><Icon name="mini" size={18} /></button
    >
    <button
      class={styles.tool}
      type="button"
      aria-label={updates.available ? 'Settings (a new version is available)' : 'Settings'}
      title={updates.available
        ? `Settings · Soundfolk ${updates.available.version} is out`
        : 'Settings'}
      bind:this={gear}
      onclick={ui.openSettings}
      ><Icon name="settings" size={18} />{#if updates.available}<span
          class={styles.dot}
          aria-hidden="true"
        ></span>{/if}</button
    >
  </div>

  <div class={styles.panel} role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
    {#if tab === 'history'}
      <HistoryPanel />
    {:else if tab === 'listen' && openedList && listTab === 'listen'}
      <ListScreen list={openedList} back="Listen" />
    {:else if tab === 'listen'}
      <ListenPanel
        {personal}
        onpersonalchanged={loadPersonal}
        ondiscover={() => (tab = 'discover')}
      />
    {:else if openedList && listTab === 'discover'}
      <ListScreen list={openedList} back="Discover" />
    {:else}
      <DiscoverPanel {personal} bind:searchField />
    {/if}
  </div>
</section>
