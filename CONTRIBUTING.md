# Contributing to Soundfolk

Thanks for wanting to contribute! This repository contains the **public client** of Soundfolk, released under the
[Mozilla Public License 2.0](LICENSE) (`SPDX-License-Identifier: MPL-2.0`).

## License of your contributions

By submitting a contribution (pull request, patch…), you agree that:

1. it is distributed under the **MPL-2.0**, like the rest of the repository;
2. the project maintainer may **redistribute it under any other free license approved by the
   [OSI](https://opensource.org/licenses)** (for example GPL-2.0-or-later, alongside the MPL-2.0), **without
   asking for prior consent**. This right allows the project's license to evolve (e.g. adding a GPL option,
   required by some app stores) without contacting every contributor again.

If you do not agree with this second point, please do not contribute.

## Developer Certificate of Origin (DCO)

Every commit must be signed off (`git commit -s`), which adds a `Signed-off-by: Your Name <email>` line.
This certifies that you comply with the [Developer Certificate of Origin 1.1](https://developercertificate.org/):

> By making a contribution to this project, I certify that:
>
> (a) The contribution was created in whole or in part by me and I have the right to submit it under the open source
> license indicated in the file; or
>
> (b) The contribution is based upon previous work that, to the best of my knowledge, is covered under an appropriate
> open source license and I have the right under that license to submit that work with modifications, whether created
> in whole or in part by me, under the same open source license (unless I am permitted to submit under a different
> license), as indicated in the file; or
>
> (c) The contribution was provided directly to me by some other person who certified (a), (b) or (c) and I have not
> modified it.
>
> (d) I understand and agree that this project and the contribution are public and that a record of the contribution
> (including all personal information I submit with it, including my sign-off) is maintained indefinitely and may be
> redistributed consistent with this project or the open source license(s) involved.

### Automatic check

A GitHub Action ([`dco.yml`](.github/workflows/dco.yml)) runs on every pull request and fails if a commit has no
`Signed-off-by` line, or if the email in that line differs from the commit author's email. Merge commits and bot
commits are ignored. To sign off commits you already made on your branch:

```bash
git rebase --signoff main
git push --force-with-lease
```

### Local check

A `commit-msg` hook ([`.githooks/commit-msg`](.githooks/commit-msg)) applies the same rule when you commit, before
anything reaches GitHub. Enable it once per clone:

```bash
git config core.hooksPath .githooks
```

## AI-generated content

You remain responsible for what you submit: review it, understand it, and do not submit code whose origin you
cannot certify as above.

## Trademark

The **Soundfolk** name and logo are **not** covered by this repository's license: a fork must use a different
name.

## Before opening a pull request

The CI ([`ci.yml`](.github/workflows/ci.yml)) runs these on every pull request; run them first:

```bash
cd src-tauri
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cd ..
npm run format
npm run lint
npm run check
npm test
```

`src/lib/bindings.ts` (the commands and the types they carry) is generated from the Rust code: a test
fails when it is out of date, and writes it again, so after changing a command or one of its types run
`cargo test bindings` in `src-tauri` and commit the new file.

New source files start with `SPDX-License-Identifier: MPL-2.0` (as a comment);
`.github/scripts/check-spdx.sh` checks it.

The CI also runs [typos](https://github.com/crate-ci/typos) (spelling, rules in `_typos.toml`),
[cargo-machete](https://github.com/bnjbvr/cargo-machete) (unused crates), and
[actionlint](https://github.com/rhysd/actionlint) and [zizmor](https://docs.zizmor.sh) (workflows).
Dependencies (advisories, licenses, duplicates) are checked by [`deps.yml`](.github/workflows/deps.yml), with
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) (`cd src-tauri && cargo deny check`).

### Pre-commit hook

A `pre-commit` hook ([`.githooks/pre-commit`](.githooks/pre-commit)) runs the fast checks on the files a commit
touches: `cargo fmt`, Prettier, typos (when installed) and the SPDX lines. It comes with the `commit-msg` hook
(`git config core.hooksPath .githooks`, see above). Skip it once with `git commit --no-verify`.

### Formatting commits and `git blame`

The commits that only reformat code are listed in `.git-blame-ignore-revs`. To have `git blame` skip them:

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```
