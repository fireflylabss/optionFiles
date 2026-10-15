# Changelog

We follow [Semantic Versioning](https://semver.org/) and [Keep a Changelog](https://keepachangelog.com/). optionFiles is a single CLI surface.

<details>
<summary>To see more about versioning, expand this.</summary>

Every version string starts with `v` (required), e.g. `v0.2.2-stable`, `v0.2.1`.

Here the installable surface is **CLI** (`optionfiles` / `fls`).

| Part | What you install | Example |
| --- | --- | --- |
| **CLI** | `optionfiles` / `fls` in the terminal | `v0.2.2-stable` |

With one surface there is no `m` in the tag and no per-surface sections — just the version notes.

Each release heading is the version and date; under it, a short summary ends with a plain sentence naming the surface and tag.

</details>

## UNRELEASED

Asynchronous browsing and batch transfers, richer previews, optional dual panes and native file drag and drop. These changes are unreleased; no release date, channel or tag has been assigned.

- Load directories, convert previews and run transfers asynchronously. Filter loaded entries in memory, refresh with Ctrl+R, and retain each directory's cursor and scroll position during the session.
- Mark files with `t` / Insert, extend a range with Shift+Up/Down, select all visible entries with Ctrl+A and clear marks with `u`.
- Toggle dual panes with `w`, switch the active source with Tab and use F5/F6 to copy/move to the other pane. F5 still refreshes in single-pane mode; Ctrl+R refreshes in either mode.
- Handle batch conflicts with overwrite, skip or keep-both choices (`o` / `s` / `k`), with `a` to apply a choice to remaining conflicts. Cancel transfers with Ctrl+C and inspect partial failures with `!`.
- Exchange multiple file URIs and GNOME copy/cut clipboard payloads with system clipboard tools. Preserve symbolic links, use native rename for same-filesystem moves, stage cross-device copies before removing sources, and reject self-copy and recursive destination paths.
- Preview folders, readable extensionless files and source code, PDF text/pages, video frames, audio metadata/cover art and archive listings. Use `p` for text/image mode and `[` / `]` to scroll; optional converters are ImageMagick, FFmpeg, Poppler and libarchive.
- Add Kitty OSC 72 native file dragging on Kitty 0.47+: `D` arms/disarms the outgoing source, and incoming local URI bundles can be dropped onto an idle directory list. Kitty 0.48.2 Wayland has a known missing drop-finish acknowledgement; direct native completion is not verified. Legacy pasted paths remain a compatibility input, not full native drag support or an unambiguous encoding for arbitrary unquoted spaces/newlines.
- Add the separately installed GTK `optionfiles-dnd` bridge: Ctrl+D opens an outgoing copy handle, and Ctrl+V opens a receive pad bound to the current directory. Incoming bundles use normal transfer/conflict handling; `ACCEPT` requires every declared item to copy successfully, otherwise `REJECT` is sent. Completion requires a helper completion event and successful exit. Standalone send/receive was exercised on Wayland with multiple paths including spaces and accented names; browser, Discord and Nautilus UI compatibility was not verified.

## v0.2.5-stable · 04/09/2026

`doctor` command, family SDK path, and agent docs. This version was made for CLI with a stable release channel on 04/09/2026 (v0.2.5-stable).

- New `fls doctor [--json]`: checks `~/.option/files` state dir, opener (`xdg-open`/`open`), clipboard (`wl-copy`/`xclip`/`pbcopy`), trash (`gio`/`trash`), and ImageMagick previews (`magick`/`convert`).
- Depend on `optionSDK` via the local family path (`../optionSDK`) in all crates instead of crates.io only, matching `optionUtils`.
- Add `VERSIONING.md` (single CLI surface) and refresh `AGENTS.md` (build/test/install checklist).

## v0.2.4-stable · 15/08/2026

Trash, clipboard, fuzzy search and wider-column alignment. This version was made for CLI with a stable release channel on 15/08/2026 (v0.2.4-stable).

- Trash deletion: `d` / `Delete` moves the selection to the system trash (`gio trash` / `trash`) with an XDG trash fallback under `~/.local/share/Trash` instead of permanent removal.
- System clipboard integration: `c` / `x` / `v` use `wl-copy` / `xclip` / `pbcopy` when available and fall back to the internal clipboard.
- `e` edits the selected file with `$EDITOR` (or `$VISUAL`).
- Fuzzy search ranking: multi-word or scattered queries on `/` / `Ctrl+F` rank subsequence matches; single words still match as a substring.
- Richer `fls info`: permissions, owner (uid/gid) and modification date.
- Middle-ellipsis path display in the location bar, keeping the current directory visible.
- Text truncation and column alignment are width-aware via `unicode-width`, fixing alignment with CJK and emoji filenames.

## v0.2.3-stable · 14/08/2026

Kitty previews inside optionTerm, with smarter preview caching. This version was made for CLI with a stable release channel on 14/08/2026 (v0.2.3-stable).

- Image previews now work inside **optionTerm** too: `TERM_PROGRAM=optionTerm` is detected, and the terminal's patched VTE accepts the file-based transmit (`t=f`) used for previews.
- Image previews are cached: unchanged frames are not resent, and position-only moves reuse the payload to avoid redraw flicker.

## v0.2.2-stable · 03/08/2026

Shared SDK error handling and release-ready persistence contract. This version was made for CLI with a stable release channel on 03/08/2026 (v0.2.2-stable).

- Adopt `optionSDK` 0.1.3 with the shared atomic-write and identity helpers.
- Surface failures while preparing `~/.option/files` instead of continuing with an unverified state directory.

## v0.2.1 · 02/08/2026

optionSDK integration and safer create/rename names. This version was made for CLI on 02/08/2026 (v0.2.1).

- Depend on published **optionSDK** 0.1.2 for marks, display name, home path, and `App::FILES.ensure()`.
- Reject `..`, absolute paths, and multi-segment names in create/rename so prompts cannot escape the current directory.
- Various other small tweaks

## [0.2.0] - 2026-07-18

### Added

- Case-insensitive file filtering with `/`; submitting an empty filter restores the full directory.
- Home navigation with `~` and previous-location toggle with `-`.
- Bounded text and source-code previews for common human-readable formats.
- `tree` / `t` CLI command with configurable recursion depth.
- AUR packaging for `optionfiles` with optional ImageMagick integration.
- GitHub Release workflow that automatically updates and publishes the AUR package.

### Changed

- Preview panel now chooses between Kitty graphics, source text and file metadata.
- Version bumped to 0.2.0.

## [0.1.0] - 2026-07-18

### Added

- Familiar keyboard aliases: `Delete` for deletion, `F2` for rename, `F5` or `Ctrl+R` for refresh, `Ctrl+F` for search and `.` for hidden files.
- Name filtering, home and previous-directory shortcuts, text previews and the `tree` CLI command.
- Initial release of **optionFiles** (*option files*).
- Dual binaries: `optionfiles` and short alias `fls`.
- Interactive alternate-screen terminal file manager.
- Keyboard navigation with arrows, Vim keys, paging and first/last jumps.
- Mouse row selection and wheel navigation.
- Responsive black & white file list with size metadata and details panel.
- Directory traversal and system application opening.
- Hidden-file toggle.
- Sorting by name, size and modification date.
- Internal clipboard with copy, cut and paste.
- Collision-safe copy names.
- File and directory creation.
- Rename flow and confirmed deletion.
- Help and input overlays.
- Plain `list` and `info` CLI commands.
- Automatic plain-output fallback outside a TTY.
- Kitty Graphics Protocol previews for PNG images.
- Optional ImageMagick conversion for JPEG, GIF, WebP, BMP and TIFF previews.
- Automatic Kitty, Ghostty and WezTerm detection with an environment override.
- Synchronized terminal rendering to prevent partial-frame flicker.
- Unit tests covering file operations, sizes, sorting, image detection and Base64 encoding.

[0.1.0]: https://github.com/fireflylabss/optionFiles/releases/tag/v0.1.0
[0.2.0]: https://github.com/fireflylabss/optionFiles/compare/v0.1.0...v0.2.0
