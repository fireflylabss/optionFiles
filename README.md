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

The CLI install provides `optionfiles` and `fls`. The optional GTK drag-and-drop helper is installed separately; see [Native drag and drop](#native-drag-and-drop).

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

## Previews

The preview panel shows bounded text, source code, directory contents and metadata. Content detection also handles readable extensionless files. On terminals with Kitty Graphics Protocol support, it can show images and converted document/media previews:

| Content | Optional helper |
|---|---|
| PNG, JPEG, GIF, WebP, BMP, TIFF and other image thumbnails | ImageMagick (`magick` or `convert`) |
| PDF text and first-page image | Poppler (`pdftotext`, `pdftoppm`) |
| Video metadata and frame | FFmpeg (`ffprobe`, `ffmpeg`) |
| Audio metadata and cover art when available | FFmpeg (`ffprobe`, `ffmpeg`) |
| Archive member listing | libarchive (`bsdtar`) |

Press `p` to switch between text and image representations, and `[` / `]` to scroll. Missing helpers are reported in the panel and are not required for browsing. Terminals without graphics support display the available text or metadata. The panel needs sufficient terminal width and can be toggled with Space. Graphics are constrained to the panel and cleared when navigating, opening overlays, resizing or quitting; unchanged frames are cached.

Kitty, Ghostty, WezTerm and optionTerm are detected for graphics. Override detection with:

```bash
OPTIONFILES_KITTY_GRAPHICS=1 fls   # force enable
OPTIONFILES_KITTY_GRAPHICS=0 fls   # force disable
```

Install the optional preview tools on Arch / CachyOS:

```bash
sudo pacman -S imagemagick ffmpeg poppler libarchive
```

## Native drag and drop

### Kitty OSC 72

Kitty 0.47 or newer provides the native OSC 72 drag-and-drop protocol. optionFiles queries protocol support rather than treating graphics support as proof of drag-and-drop support.

Press `D` to arm the outgoing native source, then drag the selected file bundle out of the directory list. Press `D` again to restore internal dragging. Incoming local file URI bundles can be dropped onto an idle directory list to copy into that pane's directory. Drops outside directory lists or while a blocking operation/overlay is active are rejected. Desktop targets decide which file bundles they accept.

Direct OSC 72 completion depends on the terminal implementation. Kitty 0.48.2's Wayland backend [destroys the drop offer without sending `wl_data_offer.finish`](https://github.com/kovidgoyal/kitty/blob/v0.48.2/glfw/wl_window.c), so an incoming copy can succeed while a GTK source reports cancellation. Native completion for this direct path has not been verified. Use the GTK helper bridge when an explicit transfer acknowledgement is needed.

Older terminals' pasted paths are only a compatibility path; they do not provide full native source/target support.

### Optional GTK helper

`optionfiles-dnd` is a separate GTK window for native file dragging on terminals without OSC 72. Building it requires the GUI crate's GTK 4.14+ and libadwaita 1.5+ development dependencies. It is not installed by the CLI-only command above:

```bash
cargo install --path crates/optionfiles-gui --bin optionfiles-dnd
```

On Arch / CachyOS, install the development libraries with `sudo pacman -S gtk4 libadwaita`. optionFiles looks for the helper beside its own executable, then on `PATH`. Ctrl+D opens its outgoing copy-only drag window even when Kitty advertises OSC 72. Drag from that window to the desktop target. When OSC 72 is unavailable, uppercase `D` also opens this window.

Ctrl+V opens an incoming GTK drop pad. Its destination is the active pane's directory when the pad opens; navigating afterward does not change that destination. Drop local files onto the pad to copy them with the normal transfer worker and conflict choices. optionFiles sends `ACCEPT` only after every declared item copies successfully, with no skips, errors or cancellation; otherwise it sends `REJECT`. Already copied files remain after a rejected partial transfer. Completion requires both the helper's explicit completion event and a successful helper exit, not just a local copy result. Ctrl+C cancels the transfer or idle helper, and exiting optionFiles closes and reaps the helper.

The helper can also run independently:

```bash
optionfiles-dnd send -- '/path/with spaces/photo.png' /path/to/notes.txt
optionfiles-dnd receive
```

The standalone `receive` window emits a file URI bundle on stdout as `DROP_BEGIN count`, one `DROP URI` line per file, then `DROP_END`. Its caller must reply with `ACCEPT` or `REJECT` on stdin. Only acknowledge acceptance after handling the bundle; receipt alone is not completion. Other events include `READY`, `DRAGGING`, `COMPLETED copy|receive`, `CANCELLED reason` and `ERROR reason`. Exit status is 0 for acknowledged completion, 2 for cancellation and 1 for an error. Escape or closing the window cancels. The helper never deletes source files.

The helper's send/receive path has been exercised on Wayland with multiple file URIs, including spaces and accented names. This does not establish compatibility with every desktop application; browser, Discord and Nautilus UI drops have not been verified.

## Requirements

- Rust 1.85+ (edition 2024) and the sibling optionSDK checkout for source builds.
- A terminal with alternate-screen support for the interactive interface.
- `xdg-open` on Linux or `open` on macOS for external applications.
- Optional clipboard, trash, preview and native-drag helpers as described above.

optionFiles uses no daemon, database, background indexer or cloud service for local browsing.

## License

Apache License 2.0. See [`LICENSE`](LICENSE).
