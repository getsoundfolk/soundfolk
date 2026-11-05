// SPDX-License-Identifier: MPL-2.0
//! Checking recipes against saved pages, and building the published list.

use crate::recipe::{Recipe, Test, SCHEMA_VERSION};
use crate::tracker::{OnAir, Policy, Tracker};
use crate::{extract, Snapshot, Timestamp};

/// What a recipe reads from a page, and what would be shown at `at`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub snapshot: Snapshot,
    /// `None` when nothing is on air (or ignored).
    pub on_air: Option<OnAir>,
    /// The station's UTC offset, when the page gives local times and it could be inferred.
    pub offset: Option<crate::clock::Offset>,
}

/// Reads `page` with `recipe` as if it had been fetched at `at` (its HTTP `Date`).
pub fn read(recipe: &Recipe, page: &str, at: Timestamp) -> Result<Reading, String> {
    let snapshot = extract::read(recipe, page).map_err(|e| e.to_string())?;
    let policy = Policy { jitter: crate::SignedDuration::ZERO, ..Policy::default() };
    let mut tracker = Tracker::new(policy, recipe.every(), None, 0);
    let plan = tracker.on_snapshot(snapshot.clone(), at, Some(at));
    let on_air = plan.publish.into_iter().next().map(|s| s.on_air);
    Ok(Reading { snapshot, on_air, offset: tracker.offset() })
}

/// Runs one fixture; returns what did not match (empty = passed).
pub fn run_test(recipe: &Recipe, test: &Test, page: &str) -> Vec<String> {
    let at = match test.at.as_deref().map(str::parse::<Timestamp>).transpose() {
        Ok(at) => at.unwrap_or_else(Timestamp::now),
        Err(e) => return vec![format!("`at`: {e}")],
    };
    let reading = match read(recipe, page, at) {
        Ok(reading) => reading,
        Err(e) => return vec![e],
    };
    let Some(on_air) = reading.on_air else {
        return vec!["nothing on air".into()];
    };
    let mut failures = Vec::new();
    let mut expect = |what: &str, expected: &Option<String>, actual: Option<String>| {
        if let Some(expected) = expected {
            if actual.as_deref() != Some(expected.as_str()) {
                failures.push(format!("{what}: expected {expected:?}, got {actual:?}"));
            }
        }
    };
    let current = &on_air.current;
    expect("artist", &test.artist, current.artist.clone());
    expect("title", &test.title, Some(current.title.clone()));
    expect("next title", &test.next_title, on_air.next.as_ref().map(|t| t.title.clone()));
    if let Some(expected) = &test.started_at {
        match expected.parse::<Timestamp>() {
            Ok(expected) if current.started_at == Some(expected) => {}
            Ok(expected) => failures.push(format!("start: expected {expected}, got {:?}", current.started_at)),
            Err(e) => failures.push(format!("`started_at`: {e}")),
        }
    }
    failures
}

/// The published list (`recipes.json`): every recipe without its tests, with its id (the file
/// name without `.toml`), which lets a user's own file replace it.
///
/// ```json
/// { "schema": 1, "recipes": [ { "id": "frequence7", "schema": 1, "name": "Fréquence 7", … } ] }
/// ```
pub fn compile(recipes: &[(String, Recipe)]) -> serde_json::Value {
    let recipes = recipes
        .iter()
        .map(|(id, recipe)| {
            let mut value = serde_json::to_value(recipe.published()).expect("recipe serializes");
            if let Some(object) = value.as_object_mut() {
                object.insert("id".into(), id.clone().into());
            }
            value
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "schema": SCHEMA_VERSION, "recipes": recipes })
}

/// Reads a published list as `(id, recipe)`. Recipes this version cannot read (newer schema,
/// unknown fields, invalid) and entries without a valid id are skipped: an old app keeps
/// working with a newer list.
pub fn load_compiled(json: &str) -> Result<Vec<(String, Recipe)>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let recipes = value.get("recipes").and_then(|r| r.as_array()).ok_or("no `recipes` list")?;
    Ok(recipes
        .iter()
        .filter_map(|r| {
            let mut object = r.as_object()?.clone();
            let id = object.remove("id")?.as_str().filter(|id| is_valid_id(id))?.to_owned();
            let recipe = serde_json::from_value::<Recipe>(object.into()).ok()?;
            recipe.problems().is_empty().then_some((id, recipe))
        })
        .collect())
}

/// A recipe id is a file name: lowercase ASCII letters, digits, `-` and `_`.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQUENCE7: &str = include_str!("../../../recipes/stations/frequence7.toml");

    #[test]
    fn published_lists_carry_ids_and_no_tests() {
        let recipe = Recipe::from_toml(FREQUENCE7).unwrap();
        let json = compile(&[("frequence7".into(), recipe.clone())]).to_string();
        assert!(!json.contains("fixtures"), "{json}");
        let loaded = load_compiled(&json).unwrap();
        assert_eq!(loaded, vec![("frequence7".to_owned(), recipe.published())]);
    }

    #[test]
    fn unreadable_entries_are_skipped() {
        let recipe = Recipe::from_toml(FREQUENCE7).unwrap();
        let mut list = compile(&[("ok".into(), recipe.clone()), ("newer".into(), recipe.clone())]);
        let entries = list["recipes"].as_array_mut().unwrap();
        entries[1]["schema"] = 2.into();
        entries.push(serde_json::json!({ "id": "unknown-field", "schema": 1, "future": true }));
        let mut no_id = entries[0].clone();
        no_id.as_object_mut().unwrap().remove("id");
        entries.push(no_id);
        let mut bad_id = entries[0].clone();
        bad_id["id"] = "../etc".into();
        entries.push(bad_id);
        let loaded = load_compiled(&list.to_string()).unwrap();
        assert_eq!(loaded.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["ok"]);
        assert!(load_compiled("{}").is_err());
    }
}
