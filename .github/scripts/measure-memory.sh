#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Memory used by a running Soundfolk: the app process and every process it started (WebKit web and network
# processes). Linux only: reads /proc/<pid>/smaps_rollup.
#
#   .github/scripts/measure-memory.sh [pid]     # default: the oldest process named "soundfolk"
#   .github/scripts/measure-memory.sh -w 60     # sample every 5 s for 60 s and print the mean and the peak
#
# Columns, in MiB:
#   RSS      resident pages, shared libraries counted in full in every process (what `ps` and `top` show)
#   PSS      resident pages, each shared page split between the processes that map it (sums correctly)
#   Private  pages only this process maps (Private_Clean + Private_Dirty), what quitting it would free
#   Swap     pages swapped out, shared ones split like PSS (SwapPss)
# The headline figure is PSS + Swap of the whole tree (docs/02-architecture.md, "Memory").
set -euo pipefail
export LC_ALL=C

window=0
if [[ ${1:-} == -w ]]; then
	window=$2
	shift 2
fi
root=${1:-$(pgrep -o -x soundfolk || true)}
[[ -n $root ]] || {
	echo "No soundfolk process." >&2
	exit 1
}

tree() { # $1 and its descendants
	echo "$1"
	local child
	for child in $(pgrep -P "$1"); do tree "$child"; done
}

# One line per process: pid rss pss private swap (KiB) name.
sample() {
	local pid
	for pid in $(tree "$root"); do
		[[ -r /proc/$pid/smaps_rollup ]] || continue
		awk -v pid="$pid" -v name="$(cat "/proc/$pid/comm")" '
			/^Rss:/ { rss = $2 } /^Pss:/ { pss = $2 } /^SwapPss:/ { swap = $2 }
			/^Private_(Clean|Dirty):/ { priv += $2 }
			END { print pid, rss, pss, priv, swap, name }' "/proc/$pid/smaps_rollup"
	done
}

table() {
	awk '{ printf "%-8s %-22s %8.1f %8.1f %8.1f %8.1f\n", $1, $6, $2/1024, $3/1024, $4/1024, $5/1024
	       r += $2; p += $3; v += $4; s += $5 }
	     END { printf "%-31s %8.1f %8.1f %8.1f %8.1f\n", "total", r/1024, p/1024, v/1024, s/1024
	           printf "PSS + Swap: %.1f MiB\n", (p + s)/1024 }'
}

printf "%-8s %-22s %8s %8s %8s %8s\n" pid process RSS PSS Private Swap
if ((window == 0)); then
	sample | table
	exit 0
fi

sample | table
for ((t = 5; t <= window; t += 5)); do
	sleep 5
	sample | awk '{ p += $3 + $5 } END { print p }'
done | awk '{ sum += $1; if ($1 > max) max = $1 }
	END { printf "Over %d samples: PSS + Swap mean %.1f MiB, peak %.1f MiB\n", NR, sum/NR/1024, max/1024 }'
