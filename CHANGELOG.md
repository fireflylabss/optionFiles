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
