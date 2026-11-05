// SPDX-License-Identifier: MPL-2.0
//! Station name search that misses no spelling variant.
//!
//! The catalog finds a name when it contains the searched text, ignoring case and accents, but it
//! compares everything else character by character: `80's` does not find `80’s`, `Radio-Canada`
//! does not find `Radio–Canada`, `Radio France` does not find `Radio  France` (all of these exist).
//! So the text is cut into fragments at those characters (spaces, quotes, dashes, dots), the
//! catalog is asked for names holding the fragments in order, and [`NameQuery::matches`] keeps
//! the names where only such characters separate them.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use super::catalog_text;

/// What to search for in station names.
#[derive(Debug, Clone, PartialEq)]
pub struct NameQuery {
    /// Fragments as typed (after [`catalog_text`]), sent to the catalog.
    fragments: Vec<String>,
    /// The same fragments after [`fold`], compared with folded names.
    folded: Vec<String>,
}

impl NameQuery {
    /// `None` when the text has no fragment (only spaces or punctuation).
    pub fn new(text: &str) -> Option<Self> {
        let fragments: Vec<String> = catalog_text(text)
            .split(is_separator)
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect();
        let folded: Vec<String> = fragments.iter().map(|f| fold(f)).collect();
        // A fragment made only of accents folds to nothing: it cannot be compared.
        if fragments.is_empty() || folded.iter().any(String::is_empty) {
            return None;
        }
        Some(Self { fragments, folded })
    }

    /// One fragment: the catalog's own comparison is exact, no filtering is needed.
    pub fn is_single(&self) -> bool {
        self.fragments.len() == 1
    }

    /// Catalog search text using `%` between the fragments. The catalog treats `%` as "any
    /// characters" (SQL `LIKE`): this is not documented, see [`super::radio_browser`] which
    /// checks that it still works before relying on it.
    pub fn wildcard_term(&self) -> String {
        self.fragments.join("%")
    }

    /// Longest fragment: what to ask the catalog for when `%` is not available.
    pub fn longest_fragment(&self) -> &str {
        self.fragments
            .iter()
            .max_by_key(|f| f.chars().count())
            .map_or("", String::as_str)
    }

    /// Whether `name` holds the fragments in order, separated only by spaces, quotes, dashes or
    /// dots (or nothing: `Rock'nRoll` matches `Rock'n'Roll`).
    ///
    /// `trust_catalog`: the catalog already found every fragment in order. If [`fold`] does not
    /// find them all (the catalog folds a letter differently, `æ` for example), the name is kept
    /// rather than dropped.
    pub fn matches(&self, name: &str, trust_catalog: bool) -> bool {
        let name = fold(name);
        if trust_catalog && !self.folded.iter().all(|f| name.contains(f.as_str())) {
            return true;
        }
        let (first, rest) = self.folded.split_first().expect("at least one fragment");
        name.match_indices(first.as_str()).any(|(start, _)| {
            let mut tail = &name[start + first.len()..];
            rest.iter().all(|fragment| {
                tail = tail.trim_start_matches(is_separator);
                match tail.strip_prefix(fragment.as_str()) {
                    Some(after) => {
                        tail = after;
                        true
                    }
                    None => false,
                }
            })
        })
    }
}

/// Characters a name may spell several ways, once [`catalog_text`] made them ASCII.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\'' | '"' | '-' | '.')
}

/// Text in the form the comparison uses: [`catalog_text`], then without case, accents or
/// full-width forms, like the catalog's own comparison.
fn fold(text: &str) -> String {
    catalog_text(text)
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> NameQuery {
        NameQuery::new(text).unwrap()
    }

    #[test]
    fn cuts_the_text_at_characters_with_several_spellings() {
        let q = query(" Wonder 80’s  Flashback ");
        assert_eq!(q.wildcard_term(), "Wonder%80%s%Flashback");
        assert_eq!(q.longest_fragment(), "Flashback");
        assert!(!q.is_single());
        assert!(query("Fréquence").is_single());
        assert_eq!(NameQuery::new(" ’ – … "), None);
    }

    #[test]
    fn finds_every_spelling() {
        let q = query("Wonder 80's Flashback");
        for name in [
            "Wonder 80's Flashback",
            "WONDER 80’S FLASHBACK",
            "Wonder\t80 ´s Flashback!",
        ] {
            assert!(q.matches(name, false), "{name}");
        }
        let q = query("Rock'n'Roll");
        for name in [
            "Rock ‘n’ Roll",
            "Rock'nRoll Museum",
            "Rock n Roll",
            "VIRGIN: Rock‘n’Roll Radio",
        ] {
            assert!(q.matches(name, false), "{name}");
        }
        assert!(query("Radio-Canada").matches("ICI Radio–Canada Première", false));
        assert!(query("radio france").matches("Radio\u{a0}\u{a0}France Inter", false));
        assert!(query("Frequence 7").matches("Fréquence 7", false));
        assert!(query("JAZZ radio").matches("ＪＡＺＺ Radio", false));
    }

    #[test]
    fn rejects_other_words_between_the_fragments() {
        let q = query("Jazz Radio");
        assert!(!q.matches("Smooth Jazz on Radio", false));
        assert!(!q.matches("Smooth Jazz on Radio", true));
        assert!(!query("Rock'n'Roll").matches("Rock and Roll", false));
    }

    #[test]
    fn keeps_a_name_the_catalog_found_but_the_fold_cannot_read() {
        // The catalog may fold `æ` to `ae`; this comparison does not.
        let q = query("Aero Radio");
        assert!(q.matches("Ærø Radio", true));
        assert!(!q.matches("Ærø Radio", false));
    }
}
