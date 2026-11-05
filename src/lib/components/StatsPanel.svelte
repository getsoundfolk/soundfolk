<!-- SPDX-License-Identifier: MPL-2.0 -->
<script lang="ts">
  import { api, errorMessage, type Count, type Stats, type StationStats } from '../api';
  import { history } from '../history.svelte';
  import { player } from '../player.svelte';
  import styles from './StatsPanel.module.css';

  const PERIODS = [
    { days: 7, label: '7 days' },
    { days: 30, label: '30 days' },
    { days: undefined, label: 'All time' },
  ] as const;
  const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  /** Below this, a station's repetition and diversity figures are noise. */
  const MIN_TRACKS_FOR_QUALITY = 20;
  /** Below this, a rate per hour is noise. */
  const MIN_HOURS_FOR_RATE = 1;

  let period = $state<number | undefined>(30);
  let stats = $state<Stats | null>(null);
  let stationOrder = $state<'likes' | 'rate'>('likes');
  // Id of the latest request: an older response is ignored.
  let requestId = 0;

  async function load(days: number | undefined) {
    const mine = ++requestId;
    try {
      const next = await api.getStats(days);
      if (mine === requestId) stats = next;
    } catch (e) {
      if (mine === requestId) player.error = errorMessage(e);
    }
  }

  $effect(() => {
    void history.version;
    load(period);
  });

  async function enableIdentification() {
    try {
      await api.setTrackIdentification(true);
      await load(period);
    } catch (e) {
      player.error = errorMessage(e);
    }
  }

  const regions = new Intl.DisplayNames(undefined, { type: 'region' });
  const country = (code: string) => {
    try {
      return regions.of(code) ?? code;
    } catch {
      return code;
    }
  };
  const month = (ym: string) =>
    new Date(`${ym}-01T12:00:00`).toLocaleDateString(undefined, {
      month: 'short',
      year: '2-digit',
    });

  const hours = (h: number) =>
    h < 1 ? `${Math.round(h * 60)} min` : `${h.toFixed(h < 10 ? 1 : 0)} h`;
  const percent = (r: number) => `${(r * 100).toFixed(r < 0.1 ? 1 : 0)} %`;
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

  const stations = $derived.by(() => {
    const list = stats?.stations ?? [];
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

  const quality = $derived(
    (stats?.stations ?? [])
      .filter((s) => s.tracks_heard >= MIN_TRACKS_FOR_QUALITY)
      .sort((a, b) => b.tracks_heard - a.tracks_heard)
      .slice(0, 10),
  );

  /** Liked tracks the artist-based figures leave out. */
  const leftOut = $derived(
    stats ? stats.identification.not_found + stats.identification.pending : 0,
  );
  const identified = $derived(stats ? stats.identification.matched > 0 : false);
  const listened = $derived(stats ? stats.totals.listening_hours > 0 : false);

  const max = (items: { count: number }[]) => Math.max(...items.map((i) => i.count), 0);
  const width = (value: number, top: number) =>
    `${top > 0 ? Math.max((value / top) * 100, 2) : 0}%`;
  const height = (value: number, top: number) => `${top > 0 ? (value / top) * 100 : 0}%`;
</script>

{#snippet bars(items: Count[], format: (c: Count) => string = (c) => c.label)}
  {@const top = max(items)}
  <ul class={styles.bars}>
    {#each items as item (item.label + (item.detail ?? ''))}
      <li
        class={styles.bar}
        title={`${format(item)}${item.detail ? ` — ${item.detail}` : ''}: ${item.count}`}
      >
        <span class={styles.label}>
          {format(item)}
          {#if item.detail}<span class={styles.detail}>{item.detail}</span>{/if}
        </span>
        <span class={styles.track}
          ><span class={styles.fill} style:width={width(item.count, top)}></span></span
        >
        <span class={styles.value}>{item.count}</span>
      </li>
    {/each}
  </ul>
{/snippet}

{#snippet columns(values: number[], labels: string[], every: number, format: (v: number) => string)}
  {@const top = Math.max(...values, 0)}
  <div class={styles.columns} style:--count={values.length}>
    {#each values as value, i (i)}
      <div class={styles.column} title={`${labels[i]}: ${format(value)}`}>
        <span class={styles.stick}
          ><span class={styles.rise} style:height={height(value, top)}></span></span
        >
        <span class={styles.tick}>{i % every === 0 ? labels[i] : ''}</span>
      </div>
    {/each}
  </div>
{/snippet}

<div class={styles.periods} role="radiogroup" aria-label="Period">
  {#each PERIODS as p (p.label)}
    <button
      class={styles.period}
      type="button"
      role="radio"
      aria-checked={period === p.days}
      onclick={() => (period = p.days)}
    >
      {p.label}
    </button>
  {/each}
</div>

{#if stats}
  <div class={styles.tiles}>
    <div class={styles.tile}>
      <span class={styles.big}>{hours(stats.totals.listening_hours)}</span>listened
    </div>
    <div class={styles.tile}>
      <span class={styles.big}>{stats.totals.tracks_heard}</span>tracks heard
    </div>
    <div class={styles.tile}><span class={styles.big}>{stats.totals.likes}</span>liked</div>
  </div>

  {#if !stats.identification.enabled}
    <div class={styles.notice}>
      <p>
        Artists, albums, genres, decades and countries need your liked tracks to be identified on
        MusicBrainz, the open music database. Only the artist and title of liked tracks are sent.
      </p>
      <button class={styles.action} type="button" onclick={enableIdentification}
        >Turn on identification</button
      >
    </div>
  {:else if stats.identification.pending > 0}
    <p class={styles.note}>
      Identifying {plural(stats.identification.pending, 'liked track')} on MusicBrainz…
    </p>
  {/if}

  {#if stats.totals.likes === 0 && !listened && stats.totals.tracks_heard === 0}
    <p class={styles.empty}>Nothing to show for this period yet.</p>
  {/if}

  {#if stations.length > 0 || (stats.stations.length > 0 && stationOrder === 'rate')}
    <section class={styles.section}>
      <header class={styles.head}>
        <h3 class={styles.title}>Stations</h3>
        <div class={styles.switch} role="radiogroup" aria-label="Rank stations by">
          <button
            type="button"
            role="radio"
            aria-checked={stationOrder === 'likes'}
            onclick={() => (stationOrder = 'likes')}>Likes</button
          >
          <button
            type="button"
            role="radio"
            aria-checked={stationOrder === 'rate'}
            onclick={() => (stationOrder = 'rate')}>Per hour</button
          >
        </div>
      </header>
      {#if stationOrder === 'rate'}
        <p class={styles.note}>
          Fairer than raw counts: the station you listen to most is not always the one you like
          most. Stations with at least {MIN_HOURS_FOR_RATE} h of listening.
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
            <li class={styles.bar} title={`${s.name}: ${meta}`}>
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

  {#if stats.identification.enabled && (identified || leftOut > 0) && stats.totals.likes > 0}
    {#if stats.artists.length > 0}
      <section class={styles.section}>
        <h3 class={styles.title}>Artists</h3>
        {@render bars(stats.artists)}
      </section>
    {/if}
    {#if stats.albums.length > 0}
      <section class={styles.section}>
        <h3 class={styles.title}>Albums</h3>
        {@render bars(stats.albums)}
      </section>
    {/if}
    {#if leftOut > 0}
      <p class={styles.note}>
        {plural(leftOut, 'liked track')} not counted in artists, albums, genres, decades and countries
        ({stats.identification.not_found} not found on MusicBrainz, {stats.identification.pending} waiting).
      </p>
    {/if}
  {/if}

  {#if stats.discovery_stations.length > 0}
    <section class={styles.section}>
      <h3 class={styles.title}>Where you discover artists</h3>
      <p class={styles.note}>Station of the first like of each artist.</p>
      {@render bars(stats.discovery_stations)}
    </section>
  {/if}

  {#if stats.new_artists_by_month.length > 1}
    <section class={styles.section}>
      <h3 class={styles.title}>New artists liked per month</h3>
      {@render columns(
        stats.new_artists_by_month.map((c) => c.count),
        stats.new_artists_by_month.map((c) => month(c.label)),
        Math.ceil(stats.new_artists_by_month.length / 6),
        (v) => plural(v, 'artist'),
      )}
    </section>
  {/if}

  {#if stats.shared_tracks.length > 0}
    <section class={styles.section}>
      <h3 class={styles.title}>Heard on several stations</h3>
      {@render bars(stats.shared_tracks)}
    </section>
  {/if}

  {#if listened}
    <section class={styles.section}>
      <h3 class={styles.title}>When you listen</h3>
      <p class={styles.note}>Listening time by hour of the day, then by day of the week.</p>
      {@render columns(
        stats.hours_by_hour,
        stats.hours_by_hour.map((_, h) => `${h}h`),
        6,
        hours,
      )}
      {@render columns(stats.hours_by_weekday, WEEKDAYS, 1, hours)}
    </section>
  {/if}

  {#if stats.decades.length > 0}
    <section class={styles.section}>
      <h3 class={styles.title}>Decades of your liked tracks</h3>
      {@render bars(stats.decades)}
    </section>
  {/if}

  {#if stats.genres.length > 0}
    <section class={styles.section}>
      <h3 class={styles.title}>Genres</h3>
      {@render bars(stats.genres)}
    </section>
  {/if}

  {#if stats.countries.length > 0}
    <section class={styles.section}>
      <h3 class={styles.title}>Artists' countries</h3>
      {@render bars(stats.countries, (c) => country(c.label))}
    </section>
  {/if}

  {#if quality.length > 0}
    <section class={styles.section}>
      <h3 class={styles.title}>Station profiles</h3>
      <p class={styles.note}>
        Stations with at least {MIN_TRACKS_FOR_QUALITY} tracks heard. Repetition: plays per distinct track.
        Diversity: distinct artists per 100 tracks. Unrecognised: titles without an artist (ads, jingles,
        missing metadata).
      </p>
      <table class={styles.table}>
        <thead>
          <tr
            ><th scope="col">Station</th><th scope="col">Repetition</th><th scope="col"
              >Diversity</th
            ><th scope="col">Unrecognised</th></tr
          >
        </thead>
        <tbody>
          {#each quality as s (s.station_id)}
            <tr>
              <th scope="row">{s.name}</th>
              <td>{s.repetition?.toFixed(1) ?? '–'}×</td>
              <td>{s.diversity?.toFixed(0) ?? '–'}</td>
              <td>{s.unrecognised_share !== null ? percent(s.unrecognised_share) : '–'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/if}
{/if}
