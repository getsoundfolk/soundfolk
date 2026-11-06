# SPDX-License-Identifier: MPL-2.0
"""Picks ~220 popular stations from Radio-Browser, at most 25 per country, one per name.

curl -s -A "Soundfolk loudness study" "https://de1.api.radio-browser.info/json/stations/topclick/600?hidebroken=true" > top.json
python3 -I stations.py top.json > stations.tsv
"""
import collections
import json
import sys

seen, per, count = set(), collections.Counter(), 0
for s in json.load(open(sys.argv[1])):
    url = s['url_resolved'] or s['url']
    if s.get('hls') or not s.get('lastcheckok') or '.m3u8' in url:
        continue
    key = s['name'].strip().lower()[:20]
    cc = s['countrycode'] or '??'
    if key in seen or per[cc] >= 25:
        continue
    seen.add(key)
    per[cc] += 1
    print('\t'.join(map(str, (s['stationuuid'], url, s['name'].strip(), cc, s['codec'], s['bitrate']))))
    count += 1
    if count >= 220:
        break
