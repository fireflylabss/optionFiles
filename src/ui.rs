use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute, queue};

use crate::app::{App, ClipboardMode};
use crate::fs::{Entry, human_size};
use crate::kitty::KittyPreview;

pub const DIM: Color = Color::DarkGrey;
pub const WHITE: Color = Color::White;
pub const BRIGHT: Color = Color::Rgb {
    r: 245,
    g: 245,
    b: 245,
};

pub struct TerminalUi {
    preview: KittyPreview,
}

impl TerminalUi {
    pub fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            cursor::Hide
        )?;
        Ok(Self {
            preview: KittyPreview::detect(),
        })
    }

    pub fn draw(&mut self, app: &mut App, prompt: Option<&Prompt>) -> Result<()> {
        draw(app, prompt, &mut self.preview)
    }
}

impl Drop for TerminalUi {
    fn drop(&mut self) {
        self.preview.shutdown();
        let _ = execute!(
            io::stdout(),
            cursor::Show,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

pub enum PromptKind {
    Folder,
    File,
    Rename,
    Delete,
    Search,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub value: String,
}

impl Prompt {
    pub fn label(&self) -> &'static str {
        match self.kind {
            PromptKind::Folder => "new folder",
            PromptKind::File => "new file",
            PromptKind::Rename => "rename",
            PromptKind::Delete => "delete? type y",
            PromptKind::Search => "filter by name · empty clears",
        }
    }
}

fn draw(app: &mut App, prompt: Option<&Prompt>, kitty: &mut KittyPreview) -> Result<()> {
    let (w, h) = terminal::size()?;
    let mut out = io::stdout();
    // Synchronized updates prevent the user from seeing a half-painted frame.
    queue!(
        out,
        Print("\x1b[?2026h"),
        cursor::MoveTo(0, 0),
        Clear(ClearType::All)
    )?;
    if w < 54 || h < 12 {
        print_at(&mut out, 2, 2, BRIGHT, true, "◆ optionFiles")?;
        print_at(
            &mut out,
            2,
            4,
            DIM,
            false,
            "terminal too small · resize to 54×12",
        )?;
        queue!(out, Print("\x1b[?2026l"))?;
        out.flush()?;
        return Ok(());
    }

    let margin = 2u16;
    let preview_w = if app.preview && w >= 92 { 30 } else { 0 };
    let list_w = w.saturating_sub(margin * 2 + preview_w + if preview_w > 0 { 3 } else { 0 });
    let rows = h.saturating_sub(10) as usize;
    if app.selected < app.scroll {
        app.scroll = app.selected;
    }
    if app.selected >= app.scroll + rows {
        app.scroll = app.selected + 1 - rows;
    }

    print_at(&mut out, margin, 1, BRIGHT, true, "◆ optionFiles")?;
    print_right(&mut out, w - margin, 1, DIM, "local · files")?;
    print_at(&mut out, margin, 2, DIM, false, "location")?;
    let path = compact_path(&app.cwd, w.saturating_sub(15) as usize);
    print_at(&mut out, margin + 10, 2, WHITE, false, &path)?;
    line(&mut out, margin, 3, w - margin * 2)?;
    print_at(&mut out, margin + 4, 4, DIM, false, "NAME")?;
    print_right(&mut out, margin + list_w, 4, DIM, "SIZE")?;

    if app.entries.is_empty() {
        print_at(&mut out, margin + 2, 6, DIM, false, "This folder is empty")?;
    }
    for (screen_row, (idx, entry)) in app
        .entries
        .iter()
        .enumerate()
        .skip(app.scroll)
        .take(rows)
        .enumerate()
    {
        let y = 5 + screen_row as u16;
        let selected = idx == app.selected;
        let marker = if selected { "›" } else { " " };
        let icon = if entry.is_dir {
            "▸"
        } else if entry.is_symlink {
            "↗"
        } else {
            "·"
        };
        let max_name = list_w.saturating_sub(17) as usize;
        let name = truncate(&entry.name, max_name);
        let label = format!("{marker} {icon} {name}");
        let meta = if entry.is_dir {
            "—".into()
        } else {
            human_size(entry.size)
        };
        if selected {
            selected_row(&mut out, margin, y, list_w, &label, &meta)?;
        } else {
            print_at(&mut out, margin, y, WHITE, false, &label)?;
            print_right(&mut out, margin + list_w, y, DIM, &meta)?;
        }
    }

    let mut image_area = None;
    if preview_w > 0 {
        let x = margin + list_w + 3;
        for y in 4..h - 4 {
            print_at(
                &mut out,
                x - 2,
                y,
                Color::Rgb {
                    r: 45,
                    g: 45,
                    b: 45,
                },
                false,
                "│",
            )?;
        }
        if let Some(entry) = app.current() {
            let image = kitty.supported() && KittyPreview::can_preview(&entry.path);
            draw_preview(&mut out, entry, x, 4, preview_w, image, h)?;
            if image {
                image_area = Some((
                    entry.path.as_path(),
                    x,
                    6,
                    preview_w,
                    h.saturating_sub(17).clamp(4, 10),
                ));
            }
        }
    }

    let footer_y = h - 4;
    line(&mut out, margin, footer_y - 1, w - margin * 2)?;
    let clip = app
        .clipboard
        .as_ref()
        .map(|c| {
            format!(
                " · {} {}",
                if c.mode == ClipboardMode::Copy {
                    "copy"
                } else {
                    "cut"
                },
                c.path.file_name().unwrap_or_default().to_string_lossy()
            )
        })
        .unwrap_or_default();
    let filter = if app.filter.is_empty() {
        String::new()
    } else {
        format!(" · filter ‘{}’", app.filter)
    };
    let state = format!(
        "{} item{} · sort {}{}{}",
        app.entries.len(),
        if app.entries.len() == 1 { "" } else { "s" },
        app.sort.label(),
        filter,
        clip
    );
    let status_width = app.status.chars().count() as u16;
    let state_width =
        w.saturating_sub(status_width + margin * 2 + if status_width > 0 { 3 } else { 0 });
    print_at(
        &mut out,
        margin,
        footer_y,
        DIM,
        false,
        &truncate(&state, state_width as usize),
    )?;
    if !app.status.is_empty() {
        print_right(&mut out, w - margin, footer_y, BRIGHT, &app.status)?;
    }
    print_at(
        &mut out,
        margin,
        footer_y + 2,
        DIM,
        false,
        &truncate(
            "↑↓ move   enter open   / search   ~ home   c/x/v clipboard   ? help",
            w.saturating_sub(4) as usize,
        ),
    )?;

    if app.help {
        overlay_help(&mut out, w, h)?;
    }
    if let Some(prompt) = prompt {
        overlay_prompt(&mut out, w, h, prompt)?;
    }
    queue!(out, Print("\x1b[?2026l"))?;
    out.flush()?;
    if app.help || prompt.is_some() {
        kitty.delete()?;
    } else if let Some((path, x, y, cols, rows)) = image_area {
        kitty.render(Some(path), x, y, cols, rows)?;
    } else {
        kitty.delete()?;
    }
    Ok(())
}

fn selected_row(
    out: &mut impl Write,
    x: u16,
    y: u16,
    width: u16,
    label: &str,
    meta: &str,
) -> Result<()> {
    let meta_len = meta.chars().count();
    let room = width.saturating_sub(meta_len as u16 + 2) as usize;
    let label = truncate(label, room);
    let gap = width as usize - label.chars().count() - meta_len;
    queue!(
        out,
        cursor::MoveTo(x, y),
        SetForegroundColor(Color::Black),
        SetBackgroundColor(BRIGHT),
        SetAttribute(Attribute::Bold),
        Print(format!("{label}{}{meta}", " ".repeat(gap))),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}

fn draw_preview(
    out: &mut impl Write,
    entry: &Entry,
    x: u16,
    y: u16,
    width: u16,
    image: bool,
    terminal_height: u16,
) -> Result<()> {
    if image {
        let rows = terminal_height.saturating_sub(17).clamp(4, 10);
        let meta_y = y + rows + 3;
        print_at(out, x, meta_y, DIM, false, "image · kitty graphics")?;
        print_at(out, x, meta_y + 1, WHITE, false, &human_size(entry.size))?;
        return Ok(());
    }
    print_at(
        out,
        x,
        y,
        BRIGHT,
        true,
        &truncate(&entry.name, width as usize),
    )?;
    print_at(
        out,
        x,
        y + 2,
        DIM,
        false,
        if entry.is_dir {
            "directory"
        } else if entry.is_symlink {
            "symbolic link"
        } else {
            "file"
        },
    )?;
    if !entry.is_dir {
        print_at(out, x, y + 3, WHITE, false, &human_size(entry.size))?;
    }
    if let Some(lines) = text_preview(&entry.path, terminal_height.saturating_sub(y + 7) as usize) {
        print_at(out, x, y + 5, DIM, false, "preview")?;
        for (index, line) in lines.iter().enumerate() {
            print_at(
                out,
                x,
                y + 6 + index as u16,
                WHITE,
                false,
                &truncate(line, width as usize),
            )?;
        }
        return Ok(());
    }
    let modified = entry
        .modified
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .map(relative_time)
        .unwrap_or_else(|| "unknown".into());
    print_at(out, x, y + 5, DIM, false, "modified")?;
    print_at(out, x, y + 6, WHITE, false, &modified)?;
    print_at(out, x, y + 8, DIM, false, "path")?;
    print_at(
        out,
        x,
        y + 9,
        WHITE,
        false,
        &truncate(&entry.path.display().to_string(), width as usize),
    )?;
    Ok(())
}

fn overlay_help(out: &mut impl Write, w: u16, h: u16) -> Result<()> {
    let width = 48.min(w - 4);
    let rows = [
        "enter / →   open",
        "← / backspace parent",
        "g / G       first / last",
        "a / .       hidden files",
        "s           cycle sort",
        "space       preview",
        "/ / ctrl+f  search / filter",
        "~ / -       home / previous",
        "c / x / v   copy / cut / paste",
        "n / N       new folder / file",
        "r / F2      rename",
        "d / del     delete (confirm)",
        "F5 / ctrl+r refresh",
        "o           open with system",
        "q / esc     quit / close",
    ];
    let height = (rows.len() as u16 + 5).min(h - 2);
    let x = (w - width) / 2;
    let y = (h - height) / 2;
    box_fill(out, x, y, width, height)?;
    print_at(out, x + 2, y + 1, BRIGHT, true, "keyboard")?;
    for (i, row) in rows.iter().enumerate() {
        print_at(
            out,
            x + 2,
            y + 3 + i as u16,
            if i < 2 { WHITE } else { DIM },
            false,
            row,
        )?;
    }
    print_at(out, x + 2, y + height - 2, DIM, false, "? close")?;
    Ok(())
}

fn text_preview(path: &Path, max_lines: usize) -> Option<Vec<String>> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "rs"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "css"
            | "html"
            | "xml"
            | "sh"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "java"
            | "kt"
            | "lua"
            | "ini"
            | "conf"
            | "log"
    ) {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(16 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    Some(
        text.lines()
            .take(max_lines.max(1))
            .map(|line| line.replace('\t', "  "))
            .collect(),
    )
}

fn overlay_prompt(out: &mut impl Write, w: u16, h: u16, prompt: &Prompt) -> Result<()> {
    let width = 52.min(w - 4);
    let x = (w - width) / 2;
    let y = h / 2 - 3;
    box_fill(out, x, y, width, 7)?;
    print_at(out, x + 2, y + 1, DIM, false, prompt.label())?;
    print_at(
        out,
        x + 2,
        y + 3,
        BRIGHT,
        true,
        &format!(
            "> {}",
            truncate(&prompt.value, width.saturating_sub(6) as usize)
        ),
    )?;
    print_at(out, x + 2, y + 5, DIM, false, "enter confirm · esc cancel")?;
    Ok(())
}

fn box_fill(out: &mut impl Write, x: u16, y: u16, w: u16, h: u16) -> Result<()> {
    for row in 0..h {
        queue!(
            out,
            cursor::MoveTo(x, y + row),
            SetForegroundColor(Color::Rgb {
                r: 18,
                g: 18,
                b: 18
            }),
            Print(" ".repeat(w as usize))
        )?;
    }
    queue!(out, ResetColor)?;
    Ok(())
}
fn line(out: &mut impl Write, x: u16, y: u16, width: u16) -> Result<()> {
    print_at(
        out,
        x,
        y,
        Color::Rgb {
            r: 42,
            g: 42,
            b: 42,
        },
        false,
        &"─".repeat(width as usize),
    )
}
fn print_at(
    out: &mut impl Write,
    x: u16,
    y: u16,
    color: Color,
    bold: bool,
    text: &str,
) -> Result<()> {
    queue!(
        out,
        cursor::MoveTo(x, y),
        SetForegroundColor(color),
        SetAttribute(if bold {
            Attribute::Bold
        } else {
            Attribute::NormalIntensity
        }),
        Print(text),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}
fn print_right(out: &mut impl Write, right: u16, y: u16, color: Color, text: &str) -> Result<()> {
    let x = right.saturating_sub(text.chars().count() as u16);
    print_at(out, x, y, color, false, text)
}
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.into()
    } else if max > 1 {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    } else {
        "…".into()
    }
}
fn compact_path(path: &Path, max: usize) -> String {
    truncate(&path.display().to_string(), max)
}
fn relative_time(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        "just now".into()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86400)
    }
}
