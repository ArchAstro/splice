# Contributing to Splice

Thanks for helping improve Splice.

## Before opening a change

- Search existing issues and pull requests.
- For substantial behavior or interface changes, open an issue first so the
  design can be discussed.
- Keep changes focused and include tests for observable behavior.

## Development setup

Splice is a Mac app. It needs macOS 13 or newer, Rust 1.90 or newer, and
ffmpeg (`brew install ffmpeg`) for import, playback and export.

```console
git clone https://github.com/ArchAstro/splice.git
cd splice
cargo build --locked
cargo test --locked --all-targets --all-features
make run
```

`make setup` installs a pre-commit hook that checks formatting. `make app`
builds `Splice.app` into `target/bundle`; `make install` builds from source
and links the Claude Code skill.

The editing core (`crates/splice-core`) has no platform code and holds most of
the tests: the timeline model and its edits, looks and motion, scores, scenes,
the story, the project file, and export plans. Put behavior there when you can
and test it there.

For changes you can only judge by looking, drive the running app with
`splice ctl`: `splice ctl frame 3 out.png` renders a frame as export draws it,
`splice ctl scene sheet --media M sheet.png` shows a scene's motion, and
`splice ctl ui`, `click`, `hover`, `drag`, `scroll`, `key` and `type` operate
the window as a person would (see the README).

Before opening a pull request, run the same checks as CI (`make
release-check` runs them all):

```console
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
cargo test --locked --all-targets --all-features
cargo audit
```

Install `cargo-audit` with `cargo install cargo-audit --locked` if needed.

## Pull requests

Explain the problem, the chosen solution, and how you verified it. Update
documentation (README and `skill/splice/SKILL.md`) when behavior or `splice
ctl` changes. By contributing, you agree that your work is licensed under the
repository's MIT license.

Be respectful and follow the [Code of Conduct](CODE_OF_CONDUCT.md).
