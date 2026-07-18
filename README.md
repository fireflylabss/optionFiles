# ◆ optionFiles

**optionFiles** (*option files*) — minimal black & white terminal file manager written in Rust.  
Fast keyboard navigation, mouse support and native image previews through the **Kitty Graphics Protocol**.

```text
◆ optionFiles                                                    local · files
location  /home/firefly/Projects
────────────────────────────────────────────────────────────────────────────
    NAME                                                                    SIZE
› ▸ optionFiles                                                                —
  ▸ optMusic                                                                  —
  · notes.md                                                              2.4 KB

────────────────────────────────────────────────────────────────────────────
3 items · sort name

↑↓ move   enter open   ← back   c/x/v clipboard   ? help
```

## Install

### Arch / CachyOS (AUR)

```bash
yay -S optionfiles
# or
paru -S optionfiles
```

### Build from source

Requires Rust **1.85+**.

```bash
git clone https://github.com/fireflylabss/optionFiles.git
cd optionFiles
cargo install --path .
```

| Command | Description |
|---|---|
| `optionfiles` | Full name |
| `fls` | Short alias |

## Usage

```bash
fls
fls ~/Downloads
fls -a ~/.config
fls open ~/Pictures
fls list ~/Projects
fls info archive.zip
fls tree ~/Projects --depth 2
fls --help
```

### Commands

| Command | Description |
|---|---|
| `fls [PATH]` | Open the interactive file manager |
| `fls open PATH` | Open a path interactively |
| `fls list PATH` | Print a plain directory listing |
| `fls info PATH` | Print file or directory information |
| `fls tree PATH` | Print a directory tree (`--depth`, default 3) |
| `fls -a PATH` | Start with hidden files visible |

When standard input or output is not attached to a terminal, `fls` automatically prints a plain listing instead of entering the TUI.

### Keyboard

| Key | Action |
|---|---|
| `↑` / `↓` or `j` / `k` | Move selection |
| `enter` / `→` | Enter directory or open file |
| `←` / backspace | Go to parent directory |
| `g` / `G` | Jump to first / last entry |
| Page Up / Page Down | Move by ten entries |
| `a` / `.` | Toggle hidden files |
| `s` | Cycle sort (`name` → `size` → `date`) |
| `space` | Toggle preview panel |
| `/` | Filter entries by name; submit empty to clear |
| `~` / `-` | Go home / return to previous location |
| `/` / `Ctrl+F` | Search or filter by name |
| `~` / `-` | Go to home / previous directory |
| `c` / `x` / `v` | Copy / cut / paste |
| `n` / `N` | Create directory / file |
| `r` / `F2` | Rename selected entry |
| `d` / `Delete` | Delete with confirmation |
| `F5` / `Ctrl+R` | Refresh the current directory |
| `o` | Open with the system application |
| `?` | Toggle help |
| `q` / Esc | Quit or close overlay |

### Mouse

| Action | Effect |
|---|---|
| Click row | Select entry |
| Scroll wheel | Move selection by three entries |

## Image previews

On terminals supporting the **Kitty Graphics Protocol**, optionFiles renders images directly inside the preview panel.

- PNG is transmitted natively with `f=100`
- JPEG, GIF, WebP, BMP and TIFF use ImageMagick when `magick` or `convert` is available
- placement is constrained to the preview panel without moving the terminal cursor
- images are removed when navigating, opening an overlay, resizing or quitting
- unsupported terminals keep the normal metadata preview

Kitty, Ghostty and WezTerm are detected automatically. Detection can be overridden with:

```bash
OPTIONFILES_KITTY_GRAPHICS=1 fls   # force enable
OPTIONFILES_KITTY_GRAPHICS=0 fls   # force disable
```

For non-PNG formats, install ImageMagick:

```bash
# Arch / CachyOS
sudo pacman -S imagemagick

# Debian / Ubuntu
sudo apt install imagemagick

# Fedora
sudo dnf install ImageMagick
```

## Features

- Files and directories always grouped predictably
- Name, size and modification-date sorting
- Hidden-file toggle
- Fast case-insensitive name filtering
- Home and previous-location navigation
- Copy, cut and paste with collision-safe names
- Create files and directories
- Rename and confirmed deletion
- System application integration through `xdg-open` / `open`
- Responsive black & white alternate-screen UI
- Flicker-free synchronized terminal frames
- Keyboard and mouse controls
- Kitty Graphics Protocol image previews
- Text and source-code previews with bounded reads
- Recursive `tree` command with configurable depth
- Plain non-interactive `list` and `info` commands

## Philosophy

optionFiles is not a desktop explorer rebuilt with text characters. It is a fast local navigator with a small and predictable surface: no daemon, database, background indexer or cloud dependency.

## Requirements

- Rust **1.85+** (edition 2024)
- A modern terminal with alternate-screen support
- `xdg-open` on Linux or `open` on macOS for external files
- Optional: Kitty Graphics Protocol-compatible terminal
- Optional: ImageMagick for non-PNG previews

## License

Apache License 2.0 — see [`LICENSE`](LICENSE).
