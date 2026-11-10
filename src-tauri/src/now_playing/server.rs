// SPDX-License-Identifier: MPL-2.0
//! The streaming server's side of web titles: its status page (Icecast, Shoutcast) for a stream
//! that announces nothing, and its API (AzuraCast) for the details of the titles a stream does
//! announce.

use std::time::Duration;

use reqwest::Url;
use soundfolk_nowplaying::adapters::{self, Adapter, Probe};
use soundfolk_nowplaying::tracker::Policy;
use soundfolk_nowplaying::Timestamp;

use super::{lock, track_info, until, Changes, Session, SessionClient, WebTitles};
use crate::audio::{Cancel, Reporter};

/// How long a stream that carries metadata gets to announce a title before the server's
/// status page is tried.
const INBAND_GRACE: Duration = Duration::from_secs(20);

/// Longest wait for the stream to open before giving up on the server's status page.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// An ICY title the server API does not know yet is looked up once more after this (the API may
/// lag behind the stream, a cache in front of it too).
const DETAILS_RETRY: Duration = Duration::from_secs(10);

/// Wait after a failed request for details.
const DETAILS_BACKOFF: Duration = Duration::from_secs(60);

impl WebTitles {
    /// For streams one of whose adapters [enriches](Adapter::enriches) titles (AzuraCast): the
    /// details of each ICY title, from the server's API, until the playback ends. One request
    /// per title (one more if the API does not know it yet), never closer than the minimum
    /// interval. Stops for good if the API does not describe the stream.
    pub(super) async fn icy_details(
        &self,
        session: &Session,
        client: &SessionClient,
        reporter: &Reporter,
        cancel: &Cancel,
    ) {
        let stream = session.stream.as_str();
        let Some(adapter) = adapters::candidates(stream, None)
            .into_iter()
            .find(|a| a.enriches())
        else {
            return;
        };
        if lock(&self.no_details).contains(stream) {
            return;
        }
        let Some(Probe { url, .. }) = adapter.probe(stream) else {
            return;
        };
        let floor = Policy::default().floor;
        // The ICY title being looked up, when to try, and the lookups left.
        let mut target: Option<(String, Timestamp, u8)> = None;
        let mut confirmed = false;
        let mut changes = Changes::new(&self.recipes, reporter, cancel);
        while !cancel.is_cancelled() {
            let now = Timestamp::now();
            let heard = reporter.stream_meta().icy_title();
            if heard.is_some() && heard.as_deref() != target.as_ref().map(|(t, ..)| t.as_str()) {
                target = heard.map(|h| (h, now, 2));
            }
            let earliest = lock(&self.last_fetch)
                .get(&url)
                .and_then(|last| last.checked_add(floor).ok());
            let due = target
                .as_ref()
                .is_some_and(|(_, at, left)| *left > 0 && *at <= now)
                && earliest.is_none_or(|e| e <= now);
            if let (true, Some((heard, at, left))) = (due, target.as_mut()) {
                lock(&self.last_fetch).insert(url.clone(), now);
                *left -= 1;
                let page = match client.fetch(&url).await {
                    Ok((page, _)) => page,
                    Err(e) => {
                        tracing::debug!(%url, "track details unavailable: {e}");
                        *left += 1; // the network, not the title
                        *at = now.checked_add(DETAILS_BACKOFF).unwrap_or(now);
                        continue;
                    }
                };
                match adapter.details(stream, &page, heard) {
                    Ok(Some(snapshot)) => {
                        if let Some(track) = snapshot.current.as_ref().and_then(track_info) {
                            let next = snapshot.next.first().and_then(track_info);
                            reporter.icy_details(heard, track, next, snapshot.show);
                        }
                        *left = 0;
                        if !confirmed {
                            confirmed = true;
                            tracing::info!(
                                adapter = adapter.name(),
                                "track details from the server's API"
                            );
                            self.update_status(session, |s| {
                                s.adapter = Some(adapter.name().to_owned())
                            });
                        }
                    }
                    // Not known yet, or not a track (jingle, live show the API does not see).
                    Ok(None) => *at = now.checked_add(DETAILS_RETRY).unwrap_or(now),
                    Err(e) if !confirmed => {
                        tracing::debug!(%url, "no track details for this stream: {e}");
                        lock(&self.no_details).insert(stream.to_owned());
                        return;
                    }
                    Err(e) => {
                        tracing::debug!(%url, "track details unavailable: {e}");
                        *at = now.checked_add(DETAILS_BACKOFF).unwrap_or(now);
                    }
                }
                continue;
            }
            // Next: a new ICY title, or the lookup to try again once due.
            let retry = target
                .as_ref()
                .filter(|(_, _, left)| *left > 0)
                .map(|(_, at, _)| {
                    let wake = earliest.map_or(*at, |e| e.max(*at));
                    until(wake, now)
                });
            changes.next(retry).await;
        }
    }

    /// The streaming server's status adapter, once the stream turned out not to announce titles.
    pub(super) async fn find_server(
        &self,
        client: &SessionClient,
        stream: &Url,
        reporter: &Reporter,
        cancel: &Cancel,
    ) -> Option<Adapter> {
        let meta = reporter.stream_meta();
        let mut changes = Changes::new(&self.recipes, reporter, cancel);
        let opened = loop {
            if cancel.is_cancelled() {
                return None;
            }
            if let Some(opened) = meta.opened() {
                break opened;
            }
            // `opened` is set by the decoder; give up if it never comes (failed connection).
            let left = OPEN_TIMEOUT.checked_sub(reporter_age(reporter))?;
            changes.next(Some(left)).await;
        };
        if opened.inband {
            // The stream can carry titles: give it time to announce one.
            while let Some(left) = INBAND_GRACE.checked_sub(opened.at.elapsed()) {
                if cancel.is_cancelled() || meta.has_inband_title() {
                    return None;
                }
                changes.next(Some(left)).await;
            }
            if meta.has_inband_title() {
                return None;
            }
        }

        let origin = origin_key(stream);
        if let Some(known) = lock(&self.servers).get(&origin) {
            return *known;
        }
        let mut found = None;
        for adapter in adapters::candidates(stream.as_str(), opened.server.as_deref()) {
            if cancel.is_cancelled() {
                return None;
            }
            let Some(probe) = adapter.probe(stream.as_str()) else {
                continue;
            };
            // A matching URL is not proof: the document must have the adapter's shape.
            if let Ok((page, _)) = client.fetch(&probe.url).await
                && adapter.read(stream.as_str(), &page).is_ok()
            {
                found = Some(adapter);
                break;
            }
        }
        lock(&self.servers).insert(origin, found);
        found
    }
}

pub(super) fn origin_key(url: &Url) -> String {
    format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        url.port_or_known_default().unwrap_or(0)
    )
}

fn reporter_age(reporter: &Reporter) -> Duration {
    reporter.stream_meta().created.elapsed()
}
