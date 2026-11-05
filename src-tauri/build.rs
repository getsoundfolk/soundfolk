// SPDX-License-Identifier: MPL-2.0
use std::path::Path;

fn main() {
    bundle_recipes();
    tauri_build::build()
}

/// Embeds every `recipes/stations/*.toml` in the binary (`bundled_recipes.rs` in `OUT_DIR`):
/// the recipes shipped with the app, used offline and before any remote list is loaded.
fn bundle_recipes() {
    // Read when the script runs, not baked in when it is compiled: a build script compiled from
    // another checkout must not bundle that checkout's recipes (or none).
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
    let dir = Path::new(&manifest_dir).join("recipes/stations");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|x| x == "toml"));
    files.sort();

    let mut code = String::from("/// Recipes shipped with the app: (file name, TOML).\npub const BUNDLED: &[(&str, &str)] = &[\n");
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_name().unwrap().to_string_lossy();
        code.push_str(&format!(
            "    ({name:?}, include_str!({:?})),\n",
            path.display().to_string()
        ));
    }
    code.push_str("];\n");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("bundled_recipes.rs");
    std::fs::write(out, code).expect("write bundled_recipes.rs");
}
