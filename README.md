# optionFiles

optionFiles is a black-and-white terminal file manager written in Rust. Use `optionfiles` or the short alias `fls` to browse local files with keyboard and mouse controls, optional dual panes and inline previews.

## Install

### Arch / CachyOS (AUR)

```bash
yay -S optionfiles
# or
paru -S optionfiles
```

### Build from source

Requires Rust 1.85+ and the sibling [optionSDK](https://github.com/fireflylabss/optionSDK) checkout at `../optionSDK`, as specified by the workspace dependency.

```bash
git clone https://github.com/fireflylabss/optionSDK.git
git clone https://github.com/fireflylabss/optionFiles.git
cd optionFiles
cargo install --path crates/optionfiles-cli
```

The CLI install provides `optionfiles` and `fls`.

## Usage

```bash
fls
fls ~/Downloads
fls -a ~/.config
fls open ~/Pictures
fls list ~/Projects
fls info archive.zip
fls tree ~/Projects --depth 2
fls doctor --json
fls --help
```

| Command | Description |
|---|---|
| `fls [PATH]` | Open the interactive file manager |
| `fls open PATH` | Open a path interactively |
| `fls list PATH` | Print a plain directory listing |
| `fls info PATH` | Print file or directory information |
| `fls tree PATH` | Print a directory tree (`--depth`, default 3) |
| `fls doctor [--json]` | Report local integration and dependency availability |
| `fls -a PATH` | Start with hidden files visible |

Without terminal input and output, `fls` prints a plain listing instead of entering the TUI.

## Keyboard and mouse

| Key | Action |
|---|---|
| `↑` / `↓` or `j` / `k` | Move selection |
| `Enter` / `→` | Enter directory or open file |
| `←` / Backspace | Go to parent directory |
| `g` / `G` | Jump to first / last entry |
| Page Up / Page Down | Page through entries |
| `t` / Insert | Toggle the current entry's mark |
| Shift+`↑` / Shift+`↓` | Mark a range |
| Ctrl+A / `u` | Mark all visible entries / clear marks |
| `/` / Ctrl+F | Edit the name filter; Esc restores the previous filter |
| `a` / `.` | Toggle hidden files |
| `s` | Cycle name, size and modification-date sorting |
| `~` / `-` | Go home / return to the previous directory |
| `w` | Toggle two directory panes |
| Tab / Shift+Tab | Switch active pane in dual-pane mode |
| `c` / `x` / `v` | Copy / cut / paste marked entries, or the current entry |
| F5 / F6 (dual pane) | Copy / move from the active pane to the other pane |
| F5 (single pane) / Ctrl+R (either mode) | Refresh the current directory |
| Ctrl+C | Cancel an active transfer or idle native helper |
| `!` | Open partial transfer errors |
| Space | Toggle preview panel |
| `p` | Switch text / image preview |
| `[` / `]` | Scroll preview |
| `D` | Arm / disarm native outgoing drag, or open the optional helper |
| Ctrl+D | Open the GTK outgoing copy handle, regardless of OSC 72 support |
| Ctrl+V | Open the GTK incoming drop pad for the current directory |
| `n` / `N` | Create directory / file |
| `r` / F2 | Rename current entry |
| `d` / Delete | Trash marked entries, or the current entry; confirm with `y` |
| `e` / `o` | Edit with `$EDITOR` / open with the system application |
| `?` | Toggle help |
| `q` / Esc | Quit or close overlay |

Click a row to select it. The wheel moves through a directory list or scrolls the preview when the pointer is over it. Drag entries within the TUI to a destination directory, then choose `c` to copy or `m` to move. This internal drag is separate from desktop-native drag and drop.

## Browsing and transfers

Directory loading, preview conversion and file transfers run asynchronously so the interface can continue handling input. Each directory retains its cursor and scroll position during the session. Filtering uses the loaded entries in memory rather than rereading the directory on each keystroke; use Ctrl+R when the filesystem changes. Single-word queries match substrings; multi-word queries use fuzzy subsequence ranking. Submit an empty filter to show all entries.

Marks apply batch operations to multiple files. In dual-pane mode, the active pane is the source for F5 copy and F6 move; the other pane is the destination.

When a destination exists, choose `o` to overwrite, `s` to skip or `k` to keep both. Toggle `a` to apply the choice to remaining conflicts, or Esc to cancel. Ctrl+C requests cancellation of the running transfer; files already transferred remain at the destination. Press `!` to inspect individual failures after a partial result.

Copies preserve symbolic links instead of copying their targets. Moves on the same filesystem use native rename; cross-filesystem moves copy successfully before removing the source. Transfer checks prevent overwriting the source itself or copying a directory into its own subtree; keep-both can create a distinct copy alongside the source. Trash uses `gio trash` or `trash` when available, with an XDG trash fallback under `~/.local/share/Trash`.

### Clipboard

`c`, `x` and `v` use `wl-copy` / `wl-paste` on Wayland or `xclip` on X11 when available, with an internal clipboard fallback. File clipboard payloads support multiple local file URIs and GNOME's `x-special/gnome-copied-files` copy/cut format. Cut data identifies a move rather than a copy.

URI lists preserve encoded spaces and other filename characters. Terminal paste/drop text is a compatibility input path, not a desktop-native drag protocol: arbitrary unquoted spaces and literal newlines cannot reliably distinguish multiple paths from characters inside a filename. Use file URI payloads or native drag and drop for unambiguous bundles.

## Requirements

- Rust 1.85+ (edition 2024) and the sibling optionSDK checkout for source builds.
- A terminal with alternate-screen support for the interactive interface.
- `xdg-open` on Linux or `open` on macOS for external applications.
- Optional clipboard, trash, preview and native-drag helpers as described above.

optionFiles uses no daemon, database, background indexer or cloud service for local browsing.

## License

Apache License 2.0. See [`LICENSE`](LICENSE).
