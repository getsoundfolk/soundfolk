<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- The Stats panel's station ranking: by likes, or by likes per hour of listening. -->
<script lang="ts">
  import { rovingKeydown } from '../a11y';
  import type { Stats, StationStats } from '../api';
  import { hours, percent, plural, width } from '../statsFormat';
  import styles from './StatsPanel.module.css';

  let { stats }: { stats: Stats } = $props();

  /** Below this, a rate per hour is noise. */
  const MIN_HOURS_FOR_RATE = 1;

  let stationOrder = $state<'likes' | 'rate'>('likes');

  const stations = $derived.by(() => {
    const list = stats.stations;
    if (stationOrder === 'likes')
      return list.filter((s) => s.likes > 0 || s.listening_hours > 0).slice(0, 10);
    return list
      .filter((s) => s.listening_hours >= MIN_HOURS_FOR_RATE && s.likes_per_hour !== null)
      .sort((a, b) => b.likes_per_hour! - a.likes_per_hour!)
      .slice(0, 10);
  });
  const stationValue = (s: StationStats) =>
    stationOrder === 'likes' ? s.likes : (s.likes_per_hour ?? 0);
  const stationMax = $derived(Math.max(...stations.map(stationValue), 0));
</script>

{#if stations.length > 0 || (stats.stations.length > 0 && stationOrder === 'rate')}
  <section class={styles.section}>
    <header class={styles.head}>
      <h3 class={styles.title}>Stations</h3>
      <div
        class={styles.switch}
        role="radiogroup"
        tabindex="-1"
        aria-label="Rank stations by"
        onkeydown={rovingKeydown}
      >
        <button
          type="button"
          role="radio"
          aria-checked={stationOrder === 'likes'}
          tabindex={stationOrder === 'likes' ? 0 : -1}
          onclick={() => (stationOrder = 'likes')}>Likes</button
        >
        <button
          type="button"
          role="radio"
          aria-checked={stationOrder === 'rate'}
          tabindex={stationOrder === 'rate' ? 0 : -1}
          onclick={() => (stationOrder = 'rate')}>Per hour</button
        >
      </div>
    </header>
    {#if stationOrder === 'rate'}
      <p class={styles.note}>
        Fairer than raw counts: the station you listen to most is not always the one you like most.
        Stations with at least {MIN_HOURS_FOR_RATE} h of listening.
      </p>
    {/if}
    {#if stations.length === 0}
      <p class={styles.note}>No station has enough listening time yet.</p>
    {:else}
      <ul class={styles.bars}>
        {#each stations as s (s.station_id)}
          {@const meta = [
            plural(s.likes, 'like'),
            s.listening_hours > 0 ? hours(s.listening_hours) : null,
            s.likes_per_hour !== null && s.listening_hours >= MIN_HOURS_FOR_RATE
              ? `${s.likes_per_hour.toFixed(1)}/h`
              : null,
            s.like_rate !== null ? `${percent(s.like_rate)} of tracks` : null,
          ]
            .filter(Boolean)
            .join(' · ')}
          <li
            class={styles.bar}
            data-peak={stationMax > 0 && stationValue(s) === stationMax}
            title={`${s.name}: ${meta}`}
          >
            <span class={styles.label}>{s.name}<span class={styles.detail}>{meta}</span></span>
            <span class={styles.track}
              ><span class={styles.fill} style:width={width(stationValue(s), stationMax)}
              ></span></span
            >
            <span class={styles.value}
              >{stationOrder === 'likes' ? s.likes : stationValue(s).toFixed(1)}</span
            >
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}
