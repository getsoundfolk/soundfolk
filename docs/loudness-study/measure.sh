#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
# $1 = uuid, $2 = url; writes series/<uuid>.txt as "t M S peakL peakR" lines
out="$(dirname "$0")/series/$1.txt"
timeout 130 ffmpeg -nostdin -hide_banner -loglevel verbose -rw_timeout 15000000 -user_agent "Soundfolk/0.1" -t 90 -i "$2" \
  -af ebur128=framelog=verbose:peak=sample -f null - 2>&1 \
  | awk '/Parsed_ebur128.* t: /{ for(i=1;i<=NF;i++){ if($i=="t:")t=$(i+1); if($i ~ /^M:/){m=substr($i,3); if(m=="")m=$(i+1)} if($i ~ /^S:/){s=substr($i,3); if(s=="")s=$(i+1)} if($i=="SPK:"){a=$(i+1);b=$(i+2)} } print t, m, s, a, b }' > "$out"
