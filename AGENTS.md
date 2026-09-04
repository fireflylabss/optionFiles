# AGENTS.md — optionFiles

## Product

**optionFiles** (*option files*) — minimal black & white terminal file manager.

Binaries: `optionfiles` · `fls` (same entrypoint). GTK: `optionfiles-gtk` · `fls-gtk`.

Local-first. No daemon. No network. Config/state via `optionSDK` (`~/.option/files`).

## After every change

When you finish a task that touches code (features, fixes, UI, deps):

1. **Build** — always verify compile:

   ```bash
   export CARGO_TARGET_DIR="$(pwd)/target"
   cargo fmt --check
   cargo test
   cargo build --release
   ```

2. **Install to PATH** — refresh local binaries so `optionfiles` / `fls` match the tree:

   ```bash
   export CARGO_TARGET_DIR="$(pwd)/target"
   cargo install --path crates/optionfiles-cli --force --offline
   ```

   Use `--offline` when deps are already fetched; drop it if the lockfile needs network.

Do **not** leave the user on a stale `~/.cargo/bin/fls` after finishing work.

## Sandbox / target dir

If builds look stale, check `CARGO_TARGET_DIR`. Prefer:

```bash
export CARGO_TARGET_DIR="$(pwd)/target"
```

## Stack notes

- Workspace: `crates/optionfiles-core` (shared state) + `crates/optionfiles-cli` (TUI) + `crates/optionfiles-gui` (GTK4/libadwaita)
- `optionSDK` via workspace path `../optionSDK` (`App::FILES`, `ensure()`, `atomic_write`, `color_enabled`, `home_dir`)
- State: `~/.option/files/` (ensure via SDK, surface failures instead of continuing unverified)
- TUI: `crossterm` alternate screen, `?` help, `q/Esc` quit, `j/k`, mouse, Kitty Graphics previews (ImageMagick optional for non-PNG)
- Colors respect `NO_COLOR` + TTY; destructive actions (`d` trash) stay confirmed

## Release channels

See [VERSIONING.md](VERSIONING.md) and blurb in [CHANGELOG.md](CHANGELOG.md). Single surface — no `m` in tag. Do **not** label `stable` unless release-ready (GitHub Release / AUR).

## Don’t

- Commit or push unless the user asks
- Force-push / skip hooks / amend pushed commits
- Regress the compact B&W minimalist UI without intent
- Bypass `App::FILES.ensure()` or write outside `~/.option/files` except XDG trash fallback
