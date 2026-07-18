# AGENTS.md — optionFiles

## Product

**optionFiles** (*option files*) — minimal terminal file manager.

Binaries: `optionfiles` · `fls` (same entrypoint).

## After every code change

```bash
export CARGO_TARGET_DIR="$(pwd)/target"
cargo fmt --check
cargo test
cargo build --release
```

Keep destructive actions confirmed and preserve the compact monochrome interface.
