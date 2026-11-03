# Contributing to Eridu

Thanks for your interest! Bug reports, ideas and pull requests are all welcome.

## Reporting a bug

Open an issue with your OS, GPU, the Eridu version (or commit) and the command you ran. If the
repository is public, link it: most bugs depend on a history's shape. Log lines with `WARN` or
`ERROR` help a lot.

## Building

You need [Rust](https://rustup.rs) (stable) and `git`. On Linux, install
`pkg-config libx11-dev libxkbcommon-dev libwayland-dev` first.

```sh
cargo build --release               # the first build takes a while: it compiles Bevy
cargo run --release -- path/to/repo
```

Always work in `release`: a real-time 3D engine needs optimized builds to run smoothly.

## Before opening a pull request

```sh
cargo fmt                               # CI checks the formatting
cargo test --release
cargo clippy --release --all-targets -- -D warnings   # CI fails on any warning
```

- **Performance is a feature.** If you touch anything that runs every frame, compare
  `eridu --bench <a large repository>` (Bevy's own history is a good one) before and after, and
  put both numbers in the pull request. Per-frame cost should follow what changes on screen,
  not the size of the repository: no allocations in hot systems, no entity per file.
- **Keep the motion calm** and **never show last names**: these are deliberate choices.
- Keep dependencies few; prefer the standard library.
- Match the existing style: small functions, doc comments explaining *why*, named constants
  for tunables.

[`AGENTS.md`](AGENTS.md) describes the architecture in detail (it is written for coding agents,
but is the best map of the code for humans too).

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
