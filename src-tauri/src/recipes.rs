// SPDX-License-Identifier: MPL-2.0
//! Now-playing recipes available to the app: the user's own (`<config dir>/recipes/*.toml`)
//! first, then those shipped with the app (`src-tauri/recipes/stations`). See
//! `docs/07-now-playing.md`.

use std::path::{Path, PathBuf};

use soundfolk_nowplaying::recipe::{LoadError, Recipe, RecipeBook};

include!(concat!(env!("OUT_DIR"), "/bundled_recipes.rs"));

/// Local recipes, then bundled ones. Invalid files are skipped (and logged).
pub fn load(local_dir: Option<&Path>) -> RecipeBook {
    let mut book = RecipeBook::default();
    if let Some(dir) = local_dir {
        book.extend(local(dir));
    }
    book.extend(BUNDLED.iter().filter_map(|(name, text)| parse(name, text)));
    book
}

fn local(dir: &Path) -> Vec<Recipe> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| match std::fs::read_to_string(path) {
            Ok(text) => parse(&path.display().to_string(), &text),
            Err(e) => {
                tracing::warn!("recipe {}: {e}", path.display());
                None
            }
        })
        .collect()
}

fn parse(name: &str, text: &str) -> Option<Recipe> {
    match Recipe::from_toml(text) {
        Ok(recipe) => Some(recipe),
        Err(e @ LoadError::NewerSchema(_)) => {
            tracing::info!("recipe {name} skipped: {e}");
            None
        }
        Err(e) => {
            tracing::warn!("recipe {name} is invalid: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_recipes_are_valid_and_pass_their_tests() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/stations");
        assert!(!BUNDLED.is_empty());
        for (name, text) in BUNDLED {
            let recipe = Recipe::from_toml(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!recipe.tests.is_empty(), "{name}: no [[test]]");
            for test in &recipe.tests {
                let page = std::fs::read_to_string(dir.join(&test.page)).unwrap();
                let failures = soundfolk_nowplaying::check::run_test(&recipe, test, &page);
                assert!(failures.is_empty(), "{name} ({}): {failures:?}", test.page);
            }
        }
    }

    #[test]
    fn published_schema_is_up_to_date() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/recipe.schema.json");
        let published: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            published,
            soundfolk_nowplaying::recipe::json_schema(),
            "regenerate: cargo run -p soundfolk-nowplaying --bin recipe-check -- schema > recipes/recipe.schema.json"
        );
    }

    #[test]
    fn local_recipes_come_first() {
        let dir = std::env::temp_dir().join(format!("soundfolk-recipes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_, bundled) = BUNDLED.iter().find(|(n, _)| *n == "frequence7.toml").unwrap();
        std::fs::write(dir.join("mine.toml"), bundled.replace("name = \"Fréquence 7\"", "name = \"Mine\"")).unwrap();
        std::fs::write(dir.join("broken.toml"), "schema = 1\nname = ").unwrap();
        let book = load(Some(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(book.find(None, "https://live.aurafm.org/Frequence7").unwrap().name, "Mine");
        assert_eq!(book.len(), 2);
    }
}
