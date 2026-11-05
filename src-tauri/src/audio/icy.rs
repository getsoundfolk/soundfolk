// SPDX-License-Identifier: MPL-2.0
//! ICY metadata (Shoutcast/Icecast): the title currently on air.
//!
//! With the `Icy-Metadata: 1` header, the server replies `icy-metaint: N` and inserts into the stream,
//! every N audio bytes, a `[length/16][text]` block (`StreamTitle='Artist - Title';`).
//! [`IcyReader`] strips these blocks so symphonia only sees audio, and reports
//! each title as it goes by.

use std::io::{self, Read};
use std::sync::Arc;

pub type TitleCallback = Arc<dyn Fn(String) + Send + Sync>;

pub struct IcyReader<R> {
    inner: R,
    metaint: usize,
    /// Audio bytes remaining before the next metadata block.
    until_meta: usize,
    block: Vec<u8>,
    on_title: TitleCallback,
}

impl<R: Read> IcyReader<R> {
    /// `metaint` must be > 0 (value of the `icy-metaint` header).
    pub fn new(inner: R, metaint: usize, on_title: TitleCallback) -> Self {
        debug_assert!(metaint > 0);
        Self {
            inner,
            metaint,
            until_meta: metaint,
            block: Vec::new(),
            on_title,
        }
    }

    fn read_metadata_block(&mut self) -> io::Result<()> {
        let mut len = [0u8; 1];
        self.inner.read_exact(&mut len)?;
        let size = usize::from(len[0]) * 16;
        if size == 0 {
            return Ok(()); // "no change"
        }
        self.block.resize(size, 0);
        self.inner.read_exact(&mut self.block)?;
        if let Some(title) = parse_stream_title(&self.block) {
            (self.on_title)(title);
        }
        Ok(())
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.until_meta == 0 {
            self.read_metadata_block()?;
            self.until_meta = self.metaint;
        }
        let max = out.len().min(self.until_meta);
        let read = self.inner.read(&mut out[..max])?;
        self.until_meta -= read;
        Ok(read)
    }
}

/// Extracts `StreamTitle` from a metadata block. `None` if missing or empty.
fn parse_stream_title(block: &[u8]) -> Option<String> {
    // Servers send UTF-8 or Latin-1 without saying which: try UTF-8 first.
    let text = match std::str::from_utf8(block) {
        Ok(s) => s.to_owned(),
        Err(_) => block.iter().map(|&b| char::from(b)).collect(),
    };
    let text = text.trim_end_matches('\0');

    const KEY: &str = "StreamTitle='";
    let rest = &text[text.find(KEY)? + KEY.len()..];
    // A title may contain an apostrophe: the real terminator is `';`.
    let end = rest.find("';").or_else(|| rest.rfind('\''))?;
    let title = rest[..end].trim();
    (!title.is_empty()).then(|| title.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Mutex;

    fn block(text: &str) -> Vec<u8> {
        let mut bytes = text.as_bytes().to_vec();
        let units = bytes.len().div_ceil(16);
        bytes.resize(units * 16, 0);
        let mut out = vec![units as u8];
        out.extend(bytes);
        out
    }

    #[test]
    fn parses_titles() {
        assert_eq!(
            parse_stream_title(b"StreamTitle='Artist - Song';StreamUrl='';\0\0"),
            Some("Artist - Song".into())
        );
        assert_eq!(
            parse_stream_title(b"StreamTitle='Guns N' Roses - Don't Cry';"),
            Some("Guns N' Roses - Don't Cry".into())
        );
        assert_eq!(parse_stream_title(b"StreamTitle='';"), None);
        assert_eq!(parse_stream_title(b"StreamUrl='x';"), None);
    }

    #[test]
    fn decodes_latin1_fallback() {
        // "Beyoncé" in Latin-1: 0xE9 is not valid UTF-8.
        assert_eq!(
            parse_stream_title(b"StreamTitle='Beyonc\xe9';"),
            Some("Beyonc\u{e9}".into())
        );
    }

    #[test]
    fn strips_metadata_and_reports_titles_even_with_tiny_reads() {
        let mut stream = Vec::new();
        stream.extend(b"AAAAAAAA");
        stream.extend(block("StreamTitle='One';"));
        stream.extend(b"BBBBBBBB");
        stream.push(0); // empty block
        stream.extend(b"CCCCCCCC");
        stream.extend(block("StreamTitle='Two';"));
        stream.extend(b"DD");

        let titles = Arc::new(Mutex::new(Vec::new()));
        let sink = titles.clone();
        let mut reader = IcyReader::new(
            Cursor::new(stream),
            8,
            Arc::new(move |t| sink.lock().unwrap().push(t)),
        );

        let mut audio = Vec::new();
        let mut chunk = [0u8; 3]; // reads smaller than metaint: the edge cases
        loop {
            match reader.read(&mut chunk).unwrap() {
                0 => break,
                n => audio.extend_from_slice(&chunk[..n]),
            }
        }
        assert_eq!(audio, b"AAAAAAAABBBBBBBBCCCCCCCCDD");
        assert_eq!(*titles.lock().unwrap(), ["One", "Two"]);
    }
}
