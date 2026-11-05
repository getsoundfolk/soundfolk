// SPDX-License-Identifier: MPL-2.0
//! `recipe-check`: validate, test and publish now-playing recipes.
//!
//! ```text
//! recipe-check validate <recipe.toml>...        check the format
//! recipe-check test <recipe.toml>...            replay each recipe's [[test]] fixtures
//! recipe-check run <recipe.toml> [--page FILE] [--at RFC3339]
//!                                               show what the recipe reads (live without --page,
//!                                               needs the `cli` feature)
//! recipe-check fixture <recipe.toml> [--page FILE] [--at RFC3339]
//!                                               save the excerpt of the page the recipe reads in
//!                                               ../fixtures/ and write the recipe's [[test]] (live
//!                                               without --page, needs the `cli` feature)
//! recipe-check compile <dir> [-o recipes.json]  build the published list
//! recipe-check schema                           print the JSON Schema
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use soundfolk_nowplaying::check;
use soundfolk_nowplaying::recipe::{json_schema, Recipe};
use soundfolk_nowplaying::Timestamp;

const USAGE: &str = "usage: recipe-check validate|test <recipe.toml>... \
    | run|fixture <recipe.toml> [--page FILE] [--at TIME] | compile <dir> [-o FILE] | schema";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("validate") => validate(&args[1..]),
        Some("test") => test(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("fixture") => fixture(&args[1..]),
        Some("compile") => compile(&args[1..]),
        Some("schema") => {
            println!("{}", serde_json::to_string_pretty(&json_schema()).expect("schema serializes"));
            Ok(())
        }
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn load(path: &Path) -> Result<Recipe, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Recipe::from_toml(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn validate(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err(USAGE.into());
    }
    let mut failed = 0;
    for path in paths {
        match load(Path::new(path)) {
            Ok(_) => println!("ok    {path}"),
            Err(e) => {
                failed += 1;
                println!("FAIL  {e}");
            }
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} invalid recipe(s)"))
    }
}

fn test(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err(USAGE.into());
    }
    let (mut passed, mut failed) = (0, 0);
    for path in paths.iter().map(Path::new) {
        let recipe = match load(path) {
            Ok(recipe) => recipe,
            Err(e) => {
                failed += 1;
                println!("FAIL  {e}");
                continue;
            }
        };
        if recipe.tests.is_empty() {
            failed += 1;
            println!("FAIL  {}: no [[test]]", path.display());
        }
        for t in &recipe.tests {
            let page_path = path.parent().unwrap_or(Path::new(".")).join(&t.page);
            let failures = match std::fs::read_to_string(&page_path) {
                Ok(page) => check::run_test(&recipe, t, &page),
                Err(e) => vec![format!("{}: {e}", page_path.display())],
            };
            if failures.is_empty() {
                passed += 1;
                println!("ok    {} ({})", path.display(), t.page);
            } else {
                failed += 1;
                println!("FAIL  {} ({})\n      {}", path.display(), t.page, failures.join("\n      "));
            }
        }
    }
    println!("{passed} passed, {failed} failed");
    if failed == 0 {
        Ok(())
    } else {
        Err("some tests failed".into())
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or(USAGE)?;
    let recipe = load(Path::new(path))?;
    let (page, at) = page_and_time(&recipe, args)?;
    let reading = check::read(&recipe, &page, at)?;
    let Some(on_air) = reading.on_air else {
        println!("nothing on air");
        return Ok(());
    };
    let current = &on_air.current;
    println!("on air:  {}", current.display());
    if let Some(start) = current.started_at {
        println!("started: {start}");
    }
    if let Some(end) = current.ends_at() {
        println!("ends:    {end}");
    }
    if let Some(next) = &on_air.next {
        let at = next.started_at.map(|s| format!(" at {s}")).unwrap_or_default();
        println!("next:    {}{at}", next.display());
    }
    if let Some(show) = &on_air.show {
        println!("show:    {show}");
    }
    Ok(())
}

/// The page (`--page`, else fetched now) and when it was read (`--at`, else its HTTP `Date`,
/// else now).
fn page_and_time(recipe: &Recipe, args: &[String]) -> Result<(String, Timestamp), String> {
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let at = option("--at").map(|at| at.parse::<Timestamp>().map_err(|e| format!("--at: {e}"))).transpose()?;
    let (page, date) = match option("--page") {
        Some(page) => (std::fs::read_to_string(page).map_err(|e| format!("{page}: {e}"))?, None),
        None => fetch(&recipe.fetch.url)?,
    };
    Ok((page, at.or(date).unwrap_or_else(Timestamp::now)))
}

fn fixture(args: &[String]) -> Result<(), String> {
    let path = Path::new(args.first().ok_or(USAGE)?);
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let recipe = load(path)?;
    let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if !check::is_valid_id(&id) {
        return Err(format!("{}: file names are lowercase letters, digits, `-` and `_`", path.display()));
    }
    let (page, at) = page_and_time(&recipe, args)?;
    let shared = soundfolk_nowplaying::fixture::prepare(&id, &text, &recipe, &recipe.fetch.url, &page, at)?;
    let dir = path.parent().unwrap_or(Path::new(".")).join("../fixtures");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let fixture_path = dir.join(&shared.fixture_file);
    std::fs::write(&fixture_path, &shared.fixture).map_err(|e| format!("{}: {e}", fixture_path.display()))?;
    std::fs::write(path, &shared.recipe).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote   {} ({} bytes)", fixture_path.display(), shared.fixture.len());
    println!("updated {} ([[test]] at {at})", path.display());
    Ok(())
}

/// The page and its HTTP `Date`.
#[cfg(feature = "cli")]
fn fetch(url: &str) -> Result<(String, Option<Timestamp>), String> {
    let agent = ureq::Agent::config_builder()
        .user_agent(concat!(
            "recipe-check/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/getsoundfolk/soundfolk-recipes)"
        ))
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .new_agent();
    let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
    let date = response
        .headers()
        .get("date")
        .and_then(|d| d.to_str().ok())
        .and_then(|d| jiff::fmt::rfc2822::DateTimeParser::new().parse_timestamp(d).ok());
    let body = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    Ok((body, date))
}

#[cfg(not(feature = "cli"))]
fn fetch(_url: &str) -> Result<(String, Option<Timestamp>), String> {
    Err("live fetching needs the `cli` feature (cargo run --features cli); or pass --page FILE".into())
}

fn compile(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or(USAGE)?);
    let output = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1));
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    // Any invalid recipe fails the build: the published list must stay readable by every app.
    let recipes = paths
        .iter()
        .map(|p| {
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if !check::is_valid_id(&id) {
                return Err(format!("{}: file names are lowercase letters, digits, `-` and `_`", p.display()));
            }
            load(p).map(|recipe| (id, recipe))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Compact: downloaded by every app (pipe through `jq .` to read it).
    let json = serde_json::to_string(&check::compile(&recipes)).expect("list serializes");
    match output {
        Some(file) => std::fs::write(file, json + "\n").map_err(|e| format!("{file}: {e}"))?,
        None => println!("{json}"),
    }
    eprintln!("{} recipe(s)", recipes.len());
    Ok(())
}
