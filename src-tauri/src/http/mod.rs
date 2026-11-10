// SPDX-License-Identifier: MPL-2.0
//! What every HTTP request of the app shares: one User-Agent, clients built once, the
//! public-only rule ([`public_net`]) and a cap on response bodies ([`body_at_most`]).
//!
//! The shared clients have a [`TIMEOUT`] by default; a request that must answer sooner sets its
//! own (`RequestBuilder::timeout`). Clients that need something else (the stream decoder's
//! headers, a pinned Radio-Browser mirror) start from [`builder`] so they keep the User-Agent.

pub mod public_net;

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{Client, ClientBuilder, Response};

/// How the app names itself to every server: some (Wikimedia, MusicBrainz) ask for a way to
/// reach its authors.
pub const USER_AGENT: &str = concat!(
    "Soundfolk/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/getsoundfolk/soundfolk)"
);
/// How the app names itself to radio stations, for their streams and playlists: an address is
/// of no use to them, and a robot-like `(+https://…)` could put off a firewall.
pub const STREAM_USER_AGENT: &str = concat!("Soundfolk/", env!("CARGO_PKG_VERSION"));
/// A request of a shared client that sets no timeout of its own gives up after this.
pub const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A client builder with the app's User-Agent.
pub fn builder() -> ClientBuilder {
    Client::builder().user_agent(USER_AGENT)
}

/// Builds a client once; later calls get the same one (or the same error).
pub fn cached(
    cell: &'static OnceLock<Result<Client, String>>,
    build: impl FnOnce() -> ClientBuilder,
) -> Result<&'static Client, String> {
    cell.get_or_init(|| {
        build()
            .build()
            .map_err(|e| format!("HTTP client unavailable: {e}"))
    })
    .as_ref()
    .map_err(Clone::clone)
}

fn shared() -> ClientBuilder {
    builder().connect_timeout(CONNECT_TIMEOUT).timeout(TIMEOUT)
}

/// The client for the app's own services and APIs (lists, catalogs, recipe pages).
pub fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    cached(&CLIENT, shared)
}

/// The client for addresses written by others (logos, covers, catalog playlists): it only
/// reaches the public Internet. Requests must still [`public_net::check`] their first address.
pub fn public_client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    cached(&CLIENT, || public_net::restrict(shared()))
}

/// Reads a response body of at most `max` bytes, stopping as soon as it is over: a server
/// announcing a size too big is refused before anything is read, and one that sends without
/// announcing (chunked) is cut at `max`, never held whole in memory.
pub(crate) async fn body_at_most(mut response: Response, max: usize) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err("response too large".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > max {
            return Err("response too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serves one response, raw, on a local port; its address.
    async fn serve(response: &'static [u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(response).await;
        });
        format!("http://{address}/")
    }

    async fn read(response: &'static [u8], max: usize) -> Result<Vec<u8>, String> {
        let url = serve(response).await;
        let response = Client::new().get(url).send().await.unwrap();
        body_at_most(response, max).await
    }

    #[tokio::test]
    async fn a_body_within_the_limit_is_read() {
        let body = read(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", 5).await;
        assert_eq!(body.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn an_announced_size_too_big_is_refused() {
        let body = read(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nhello!", 5).await;
        assert!(body.is_err());
    }

    #[tokio::test]
    async fn an_unannounced_body_is_cut_at_the_limit() {
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n4\r\nefgh\r\n0\r\n\r\n";
        assert!(read(chunked, 5).await.is_err());
        assert_eq!(read(chunked, 8).await.unwrap(), b"abcdefgh");
    }
}
