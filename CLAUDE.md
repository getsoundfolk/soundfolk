# Soundfolk: notes for coding agents

Tauri 2 desktop app: Rust back end in `src-tauri/` (edition 2024), Svelte 5 front end in `src/`. Read
[CONTRIBUTING.md](CONTRIBUTING.md) and the doc of the area you touch (`docs/`). Code, comments and docs
are in English.

## Rules the audits keep finding broken (docs/99-analysis.md)

Some are checked by tools (in brackets); the others depend on you.

- **No SQLite, file or network wait on an async worker or under a lock.** Database work goes through
  `state::with_db` / `state::blocking`, or `state::spawn_blocking` when nothing awaits it. Never use
  the database while holding the player's `Status` lock. [Debug builds panic, so the tests fail;
  clippy refuses `spawn_blocking` called directly.]
- **HTTP clients come from `http`.** Use `http::client()` for the app's own services and
  `http::public_client()` plus `public_net::check` for any address written by others (logos, covers,
  playlists, recipe pages). Build a special client from `http::builder()`. Cap bodies with
  `http::body_at_most`. [clippy refuses `reqwest::Client::new` / `builder`.]
- **No swallowed errors.** A failure the user or a developer should know about is logged
  (`tracing::warn!`) or shown (`errors.report`). `let _ =` on a `Result` needs a comment that says
  why the failure does not matter. [ESLint refuses a `.catch` whose handler ignores the error,
  unless an `eslint-disable-next-line no-restricted-syntax -- why` comment comes before it.]
- **Stores change only through their own methods.** They are exported as `Readonly<…>`, so writing
  `player.x = …` from a component does not type-check. Add a method instead of casting it away.
- **Files stay small.** At most 800 lines of Rust before `mod tests`, and 350 lines per component.
  Split the file rather than raise the limit.
  [`.github/scripts/check-file-sizes.sh`, run by the CI.]
- **Visual design (docs/07-visual-design.md):** colors and durations come from `var(--…)`. Radii and
  font weights stay on the scale of §3 and §5. Every animation has a
  `prefers-reduced-motion: reduce` fallback. Icons go through `Icon.svelte`. Nothing shifts sideways
  with its content. [Stylelint checks all but the icons, in `npm run lint`.]
- **New files are created atomically** (`files::save_download` and its siblings), never with
  `exists()` then `write`.

## Before committing

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace --all-features
npm run check && npm run lint && npm test && npx prettier --check . && .github/scripts/check-file-sizes.sh
```

- Run `rustfmt --edition 2024` on the files you changed, never a repo-wide `cargo fmt`. The pre-commit
  hook checks formatting.
- Sign off every commit (`git commit -s`). Main refuses force pushes.
- Other sessions work in this checkout: commit only your own files.
