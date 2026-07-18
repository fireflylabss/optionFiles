# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
