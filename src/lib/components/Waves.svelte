<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Soundfolk's default art, shown when there is neither an album cover nor a station logo: one
     sine wave in orange among copies of it in white, each shifted a step down the diagonal, on
     charcoal. It fills its tile, whatever its shape. -->
<script lang="ts">
  const SIZE = 1500;
  const CENTER = 747; // the orange wave's middle line
  const AMPLITUDE = 277;
  const WAVELENGTH = 1160;
  const TROUGH = 500; // x of the orange wave's lowest point
  // Each copy moves this far along the diagonal: right, and down by 0.39 of it.
  const STEP = 52;
  const SLOPE = 0.39;
  const COPIES = { before: 8, after: 12 };

  function wave(shift: number): string {
    const dx = shift * STEP;
    const dy = dx * SLOPE;
    const points: string[] = [];
    for (let x = -100; x <= SIZE + 100; x += 30) {
      const y = CENTER + dy + AMPLITUDE * Math.cos((2 * Math.PI * (x - dx - TROUGH)) / WAVELENGTH);
      points.push(`${x},${y.toFixed(1)}`);
    }
    return `M${points.join('L')}`;
  }

  const white = Array.from(
    { length: COPIES.before + COPIES.after },
    (_, i) => i - COPIES.before + (i >= COPIES.before ? 1 : 0),
  ).map(wave);
  const orange = wave(0);
</script>

<svg
  class="waves"
  viewBox="0 0 {SIZE} {SIZE}"
  preserveAspectRatio="xMidYMid slice"
  aria-hidden="true"
>
  <rect width={SIZE} height={SIZE} fill="var(--color-brand-ink)" />
  <g fill="none">
    {#each white as d, i (i)}
      <path
        {d}
        stroke="var(--color-brand-line)"
        stroke-width="0.75"
        vector-effect="non-scaling-stroke"
      />
    {/each}
    <path
      d={orange}
      stroke="var(--color-brand)"
      stroke-width="2"
      vector-effect="non-scaling-stroke"
    />
  </g>
</svg>

<style>
  .waves {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
