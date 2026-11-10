// SPDX-License-Identifier: MPL-2.0
//! Web pages read as text: in the charset they are written in, with the site's clock, and no
//! bigger than [`MAX_PAGE_BYTES`] (recipe pages, streaming servers' status pages and APIs).

use std::time::Duration;

use jiff::Timestamp;
use reqwest::{header, Client};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Larger pages are not read (a playlist page is a few dozen KB).
pub const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;

/// The page and its HTTP `Date` (the site's clock, used to resolve local times).
pub async fn fetch(client: &Client, url: &str) -> Result<(String, Option<Timestamp>), String> {
    let response = client
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let charset = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|t| t.to_str().ok())
        .and_then(charset_param)
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()));
    let date = response
        .headers()
        .get(header::DATE)
        .and_then(|d| d.to_str().ok())
        .and_then(|d| {
            jiff::fmt::rfc2822::DateTimeParser::new()
                .parse_timestamp(d)
                .ok()
        });
    let body = crate::http::body_at_most(response, MAX_PAGE_BYTES).await?;
    Ok((decode(&body, charset), date))
}

/// The page as text, in the charset it is written in, chosen as a browser would: a byte order
/// mark, then the `Content-Type` header, then a `<meta>` or `<?xml?>` declaration near the
/// start; without any, UTF-8 when the bytes are valid UTF-8, windows-1252 (Latin-1's superset)
/// otherwise.
fn decode(body: &[u8], header: Option<&'static encoding_rs::Encoding>) -> String {
    use encoding_rs::{UTF_8, WINDOWS_1252};
    let encoding = header
        .or_else(|| declared_charset(body))
        .unwrap_or_else(|| {
            if std::str::from_utf8(body).is_ok() {
                UTF_8
            } else {
                WINDOWS_1252
            }
        });
    // `decode` lets a byte order mark override the choice, and replaces invalid sequences.
    encoding.decode(body).0.into_owned()
}

/// The `charset` parameter of a `Content-Type` value.
fn charset_param(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']))
    })
}

/// The charset an HTML page (`<meta charset>`, `<meta http-equiv content="…; charset=…">`) or
/// an XML document (`<?xml … encoding="…"?>`) declares in its first kilobyte.
fn declared_charset(body: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let head = body[..body.len().min(1024)].to_ascii_lowercase();
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    // The label after `key=` in `tag`, quoted or not.
    let label = |tag: &[u8], key: &[u8]| {
        let rest = &tag[find(tag, key)? + key.len()..];
        let rest = rest
            .strip_prefix(b"\"")
            .or(rest.strip_prefix(b"'"))
            .unwrap_or(rest);
        let end = rest
            .iter()
            .position(|b| !(b.is_ascii_alphanumeric() || b"-_:.".contains(b)))
            .unwrap_or(rest.len());
        encoding_rs::Encoding::for_label(&rest[..end])
    };
    if head.starts_with(b"<?xml") {
        return label(&head[..find(&head, b"?>")?], b"encoding=");
    }
    let mut rest = &head[..];
    while let Some(start) = find(rest, b"<meta") {
        rest = &rest[start..];
        let end = rest.iter().position(|&b| b == b'>').unwrap_or(rest.len());
        if let Some(encoding) = label(&rest[..end], b"charset=") {
            // A page cannot declare UTF-16 from inside itself: what it says is read as UTF-8.
            let utf16 = encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE;
            return Some(if utf16 { encoding_rs::UTF_8 } else { encoding });
        }
        rest = &rest[end..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_read_in_their_charset() {
        let latin1 = b"<p>\xc0 l'\xe9t\xe9</p>";
        assert_eq!(decode(latin1, None), "<p>À l'été</p>");
        let utf8 = "<p>Café</p>".as_bytes();
        assert_eq!(decode(utf8, None), "<p>Café</p>");

        let header = charset_param("text/html; Charset=\"ISO-8859-1\"")
            .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()));
        assert_eq!(header, Some(encoding_rs::WINDOWS_1252));
        assert_eq!(charset_param("text/html"), None);

        let meta = b"<html><head><meta charset=\"iso-8859-15\"><title>\xa4</title>";
        assert_eq!(
            decode(meta, None),
            "<html><head><meta charset=\"iso-8859-15\"><title>€</title>"
        );
        let equiv = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\">\x93x\x94";
        assert!(decode(equiv, None).ends_with("“x”"));
        let xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>\xe9</t>";
        assert!(decode(xml, None).ends_with("<t>é</t>"));
        // The header wins over the page; a byte order mark over both.
        assert!(decode(meta, Some(encoding_rs::UTF_8)).contains('\u{fffd}'));
        assert_eq!(
            decode(
                b"\xef\xbb\xbf\xc3\xa9t\xc3\xa9",
                Some(encoding_rs::WINDOWS_1252)
            ),
            "été"
        );
        // UTF-16 declared inside the page is read as UTF-8.
        assert_eq!(
            decode("<meta charset=utf-16>é".as_bytes(), None),
            "<meta charset=utf-16>é"
        );
    }

    #[tokio::test]
    async fn pages_come_with_the_site_clock_and_their_charset() {
        let page = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=iso-8859-1\r\n\
            Date: Sun, 08 Nov 2026 12:00:00 GMT\r\nContent-Length: 4\r\n\r\n\xe9t\xe9!";
        let url = crate::http::serve(page.to_vec()).await;
        let (text, date) = fetch(&Client::new(), &url).await.unwrap();
        assert_eq!(text, "été!");
        assert_eq!(date, Some("2026-11-08T12:00:00Z".parse().unwrap()));
    }

    #[tokio::test]
    async fn errors_and_pages_too_large_are_refused() {
        let missing =
            crate::http::serve(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec())
                .await;
        assert_eq!(
            fetch(&Client::new(), &missing).await.unwrap_err(),
            "HTTP 404 Not Found"
        );
        let mut huge = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_PAGE_BYTES + 1
        )
        .into_bytes();
        huge.resize(huge.len() + MAX_PAGE_BYTES + 1, b'x');
        let url = crate::http::serve(huge).await;
        assert_eq!(
            fetch(&Client::new(), &url).await.unwrap_err(),
            "response too large"
        );
    }
}
