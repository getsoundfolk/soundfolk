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
}

/// Reads `page` with `recipe` as if it had been fetched at `at` (its HTTP `Date`).
pub fn read(recipe: &Recipe, page: &str, at: Timestamp) -> Result<Reading, String> {
    let snapshot = extract::read(recipe, page).map_err(|e| e.to_string())?;
    let policy = Policy { jitter: crate::SignedDuration::ZERO, ..Policy::default() };
    let mut tracker = Tracker::new(policy, recipe.every(), None, 0);
    let plan = tracker.on_snapshot(snapshot.clone(), at, Some(at));
    let on_air = plan.publish.into_iter().next().map(|s| s.on_air);
    Ok(Reading { snapshot, on_air })
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

/// The published list: every recipe without its tests.
pub fn compile(recipes: &[Recipe]) -> serde_json::Value {
    serde_json::json!({
        "schema": SCHEMA_VERSION,
        "recipes": recipes.iter().map(Recipe::published).collect::<Vec<_>>(),
    })
}

/// Reads a published list; recipes this version cannot read are skipped.
pub fn load_compiled(json: &str) -> Result<Vec<Recipe>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let recipes = value.get("recipes").and_then(|r| r.as_array()).ok_or("no `recipes` list")?;
    Ok(recipes
        .iter()
        .filter_map(|r| serde_json::from_value::<Recipe>(r.clone()).ok())
        .filter(|r| r.problems().is_empty())
        .collect())
}
