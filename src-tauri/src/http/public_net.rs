// SPDX-License-Identifier: MPL-2.0
//! Streams that must stay on the public Internet (docs/16-sharing.md §6.2).
//!
//! A catalog station's address is written by anyone in Radio-Browser, and a station received in a
//! link by whoever sent it: without a rule, either could make the listener's computer send a
//! request to their router or to a service on their own machine. Such streams may only reach
//! public addresses. The check is made on the addresses a name resolves to, at each connection
//! (a name that resolves to a public address once and a private one afterwards gets nothing), on
//! each redirect, and on each address inside a playlist.
//!
//! Stations typed by hand in "Add a station by URL" keep reaching the local network: a radio on
//! the listener's own network is a reason to add one.

use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use reqwest::{ClientBuilder, Url};

/// What the listener reads when a stream is refused.
pub const LOCAL_NETWORK: &str =
    "This station's address points to a local network, which stations from the catalog or from a link may not reach";

/// Most redirects followed, as reqwest does by default.
const MAX_REDIRECTS: usize = 10;

/// Whether an address is on the public Internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let [first, second, ..] = a.octets();
            !(a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                || first == 0
                // Shared address space (carrier-grade NAT), 100.64.0.0/10.
                || (first == 100 && (64..128).contains(&second)))
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = a.segments()[0];
            !(a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                // Unique local, fc00::/7.
                || (first & 0xfe00) == 0xfc00
                // Link-local, fe80::/10.
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// Names that only mean something on a local network.
fn is_local_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    !host.contains('.')
        || host == "localhost"
        || [".local", ".localhost", ".internal", ".home.arpa", ".lan"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

/// Checks an address before connecting: a web address whose host is not a local name nor a
/// non-public IP. Names are checked again once resolved ([`restrict`]).
pub fn check(url: &Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("Unsupported protocol: {}", url.scheme()));
    }
    let public = match url.host() {
        Some(url::Host::Domain(name)) => !is_local_name(name),
        Some(url::Host::Ipv4(ip)) => is_public(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_public(IpAddr::V6(ip)),
        None => false,
    };
    if public {
        Ok(())
    } else {
        Err(LOCAL_NETWORK.to_owned())
    }
}

/// A client that only reaches public addresses: names resolved to public addresses only,
/// redirects checked. Requests must still [`check`] their first address, which may be an IP
/// (an IP is never resolved).
pub fn restrict(builder: ClientBuilder) -> ClientBuilder {
    let builder = builder.redirect(reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        match check(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(message) => attempt.error(message),
        }
    }));
    // Through a proxy, the proxy resolves the station's name, and the proxy itself is often on
    // the local network: filtering its address would cut every stream. The names and IPs in the
    // addresses are still checked.
    if behind_proxy() {
        builder
    } else {
        builder.dns_resolver(Arc::new(PublicOnly))
    }
}

fn behind_proxy() -> bool {
    ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"]
        .iter()
        .flat_map(|name| [name.to_string(), name.to_ascii_lowercase()])
        .any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
}

/// Resolves names as the system does, keeping only public addresses.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = found.into_iter().filter(|a| is_public(a.ip())).collect();
            if public.is_empty() {
                return Err(Box::<dyn Error + Send + Sync>::from(LOCAL_NETWORK));
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(url: &str) -> bool {
        check(&url.parse().unwrap()).is_ok()
    }

    #[test]
    fn public_addresses_pass() {
        for url in [
            "https://icecast.radiofrance.fr/fip-midfi.mp3",
            "http://51.75.1.2:8000/live",
            "http://[2001:4860:4860::8888]/live",
            "http://100.128.0.1/live",
        ] {
            assert!(checked(url), "{url}");
        }
    }

    #[test]
    fn local_addresses_and_names_do_not() {
        for url in [
            "http://localhost:8000/live",
            "http://127.0.0.1/live",
            "http://10.0.0.5/",
            "http://172.16.3.4/",
            "http://192.168.1.1/admin",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fd12:3456::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:192.168.1.1]/",
            "http://router/",
            "http://nas.local:8000/",
            "http://printer.home.arpa/",
            "http://radio.localhost/",
            "ftp://example.org/live",
        ] {
            assert!(!checked(url), "{url}");
        }
    }

    #[tokio::test]
    async fn names_resolving_to_the_local_network_are_refused() {
        use reqwest::dns::Resolve;
        let resolved = PublicOnly.resolve("localhost".parse().unwrap()).await;
        assert!(resolved.is_err());
    }
}
