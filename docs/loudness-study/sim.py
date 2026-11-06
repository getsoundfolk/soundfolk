# SPDX-License-Identifier: MPL-2.0
"""Replays audio/loudness.rs on measured series (M, S, peak every 100 ms), block by block.

Usage: python3 -I sim.py <series dir>
"""
import math
import os
import statistics
import sys

BLOCK = 0.1


def load(path):
    rows = []
    for line in open(path):
        parts = line.split()
        if len(parts) < 5:
            continue
        try:
            t, m, s, a, b = map(float, parts[:5])
        except ValueError:
            continue
        rows.append((m, s, max(a, b)))
    return rows


def integrated(rows):
    """R128 integrated loudness over the whole series (400 ms blocks = M values, gated)."""
    ms = [m for m, _, _ in rows if m > -70]
    if not ms:
        return None
    e = [10 ** (m / 10) for m in ms]
    rel = 10 * math.log10(sum(e) / len(e)) - 10
    kept = [x for x, m in zip(e, ms) if m > rel]
    return 10 * math.log10(sum(kept) / len(kept))


DEFAULT = dict(
    target=-16.0, guess=-11.0, max_cut=-15.0, max_boost=6.0, max_boost_learning=3.0, ceiling=-1.0,
    abs_gate=-50.0, rel_gate=10.0, rel_give_up=30.0, learning=10.0, first=1.5,
    tau_learning=1.0, tau_louder=2.0, tau_quieter=20.0,
    rate_cut=6.0, rate_boost=0.5, rate_learning=6.0,
)


def run(rows, p, stored=None):
    """Gains (dB) after each block; and the perceived level each block (S or M + gain)."""
    est = stored if stored is not None else p['guess']
    phase = 'steady' if stored is not None else 'first'
    energy, blocks = 0.0, 0
    gated = 0.0
    below = 0.0
    fed = 0.0
    peaks = []

    def target(learning):
        g = min(p['target'] - est, p['max_boost_learning'] if learning else p['max_boost'])
        if g > 0 and peaks:
            pk = max(peaks[-100:])
            g = min(g, max(0.0, p['ceiling'] - pk))
        return max(g, p['max_cut'])

    gain = target(phase != 'steady')
    gains = []
    for m, s, pk in rows:
        # Ramp during the block toward the target set at the end of the previous one.
        learning = phase != 'steady'
        tg = target(learning)
        rate = p['rate_cut'] if tg < gain else (p['rate_learning'] if learning else p['rate_boost'])
        step = rate * BLOCK
        gain = min(gain + step, tg) if tg > gain else max(gain - step, tg)
        gains.append(gain)
        # End of block: measure.
        peaks.append(pk)
        fed += BLOCK
        if phase in ('first', 'learning'):
            if fed >= 0.4 and m >= p['abs_gate']:
                gated += BLOCK
                if phase == 'first':
                    energy += 10 ** (m / 10)
                    blocks += 1
                    if gated >= p['first'] - 1e-9:
                        est = 10 * math.log10(energy / blocks)
                        phase = 'learning'
                else:
                    est += (m - est) * (1 - math.exp(-BLOCK / p['tau_learning']))
                    if gated >= p['learning'] - 1e-9:
                        phase = 'steady'
        elif fed >= 3.0 and s >= p['abs_gate']:
            if s < est - p['rel_gate']:
                below += BLOCK
                if below < p['rel_give_up']:
                    continue
            else:
                below = 0.0
            gated += BLOCK
            tau = p['tau_louder'] if s > est else p['tau_quieter']
            est += (s - est) * (1 - math.exp(-BLOCK / tau))
    return gains


def metrics(stations, p):
    """Per-setting summary over all stations, each played as unknown."""
    excess = []
    over, under, settle, move, ripple, spread, unboosted = [], [], [], [], [], [], 0
    for name, rows, integ in stations:
        g = run(rows, p)
        end = statistics.median(g[-100:])
        # First 5 s: how much louder / quieter than where the gain ends.
        early = g[:50]
        over.append(max(x - end for x in early))
        excess.append(sum(max(0.0, x - end) for x in g[:100]) * BLOCK)
        under.append(min(x - end for x in early))
        last_bad = max((i for i in range(len(g)) if abs(g[i] - end) > 1.0), default=-1)
        settle.append((last_bad + 1) * BLOCK)
        tail = g[200:]
        if len(tail) > 100:
            move.append(sum(abs(a - b) for a, b in zip(tail, tail[1:])) / (len(tail) * BLOCK / 60))
            ripple.append(max(tail) - min(tail))
        st = [rows[i][1] + g[i] for i in range(max(0, len(g) - 300), len(g)) if rows[i][1] > -50]
        if st:
            spread.append(10 * math.log10(sum(10 ** (x / 10) for x in st) / len(st)) - p['target'])
        if integ < p['target'] - 1 and end < p['target'] - integ - 1:
            unboosted += 1

    def q(xs, f):
        xs = sorted(xs)
        return xs[min(len(xs) - 1, int(f * len(xs)))]

    return dict(excess=statistics.mean(excess), over_p90=q(over, 0.9), over_max=max(over), under_p10=q(under, 0.1),
                settle_med=statistics.median(settle), settle_p90=q(settle, 0.9),
                move=statistics.median(move), ripple=q(ripple, 0.9),
                spread=(q(spread, 0.1), q(spread, 0.9)), unboosted=unboosted)


def main():
    folder = sys.argv[1]
    stations = []
    for f in sorted(os.listdir(folder)):
        rows = load(os.path.join(folder, f))
        if len(rows) < 400:  # under 40 s of audio: failed or dropped
            continue
        integ = integrated(rows)
        if integ is None or integ < -45:
            continue
        stations.append((f, rows, integ))
    loud = sorted(i for _, _, i in stations)
    n = len(loud)
    print(f"{n} stations measured")
    print("integrated loudness, LUFS: p10 %.1f  p25 %.1f  median %.1f  p75 %.1f  p90 %.1f  min %.1f  max %.1f" % (
        loud[n // 10], loud[n // 4], loud[n // 2], loud[3 * n // 4], loud[9 * n // 10], loud[0], loud[-1]))
    buckets = {}
    for x in loud:
        b = int(math.floor(x / 2) * 2)
        buckets[b] = buckets.get(b, 0) + 1
    for b in sorted(buckets):
        print(f"  {b:>4} … {b + 2:>4}  {'#' * buckets[b]} {buckets[b]}")
    peaks = sorted(max(r[2] for r in rows) for _, rows, _ in stations)
    print("max sample peak, dBFS: median %.1f  p90 %.1f" % (peaks[n // 2], peaks[9 * n // 10]))
    for t in (-16, -14):
        need_boost = sum(1 for x in loud if x < t)
        capped = sum(1 for x in loud if t - x > 6)
        print(f"target {t}: {need_boost} stations need a boost, {capped} more than 6 dB")

    off = [x for _, _, x in stations]
    print("no normalization: level p10..p90 relative to the median %+.1f..%+.1f" % (loud[n // 10] - loud[n // 2], loud[9 * n // 10] - loud[n // 2]))
    variants = {'current': {}}
    for g in (-9.0, -10.0, -11.0):
        for f in (1.5, 2.0, 3.0):
            variants[f'guess {g:.0f} first {f}'] = dict(guess=g, first=f)
    for lo in (2.0, 4.0, 6.0, 8.0, 12.0):
        for qu in (20.0, 30.0, 40.0, 60.0):
            variants[f'louder {lo:.0f} quieter {qu:.0f}'] = dict(tau_louder=lo, tau_quieter=qu)
    print()
    for name, change in variants.items():
        p = dict(DEFAULT, **change)
        r = metrics(stations, p)
        print(f"{name:17} dB.s too loud {r['excess']:4.1f} | first 5 s: louder p90 {r['over_p90']:+4.1f} max {r['over_max']:+5.1f} quieter p10 {r['under_p10']:+5.1f}"
              f" | settled med {r['settle_med']:4.1f}s p90 {r['settle_p90']:4.1f}s | after 20 s: {r['move']:4.1f} dB/min, range p90 {r['ripple']:3.1f}"
              f" | level p10..p90 {r['spread'][0]:+.1f}..{r['spread'][1]:+.1f} | left quiet {r['unboosted']}")


main()
