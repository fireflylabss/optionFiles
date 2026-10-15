use std::io::{self, Write};
use std::path::Path;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute, queue};
use optionfiles_core::{ClipboardMode, human_size};

use crate::interaction::Session;
use crate::kitty::KittyPreview;
use crate::preview::{PreviewService, PreviewState};

const DIM: Color = Color::Grey;
const WHITE: Color = Color::White;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
impl Rect {
    pub fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && x < self.x.saturating_add(self.width)
            && y >= self.y
            && y < self.y.saturating_add(self.height)
    }
}

pub struct TerminalUi {
    kitty: KittyPreview,
    preview: PreviewService,
    pub lists: Vec<Rect>,
    preview_rect: Rect,
    preview_generation: u64,
    preview_text: bool,
}

impl TerminalUi {
    pub fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            cursor::Hide
        ) {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self {
            kitty: KittyPreview::detect(),
            preview: PreviewService::new(),
            lists: Vec::new(),
            preview_rect: Rect::default(),
            preview_generation: 0,
            preview_text: false,
        })
    }
    pub fn suspend(&mut self) -> Result<()> {
        self.kitty.delete()?;
        execute!(
            io::stdout(),
            Print("\x1b[?2026l"),
            cursor::Show,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
        terminal::disable_raw_mode()?;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        terminal::enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            cursor::Hide
        )?;
        Ok(())
    }

    pub fn toggle_preview_text(&mut self) {
        self.preview_text = !self.preview_text;
    }

    pub fn poll_preview(&mut self) -> bool {
        self.preview.poll()
    }
    pub fn scroll_preview(&mut self, delta: isize) {
        self.preview_text = true;
        self.preview
            .scroll(delta, self.preview_rect.height.saturating_sub(2) as usize);
    }
    pub fn preview_contains(&self, x: u16, y: u16) -> bool {
        self.preview_rect.contains(x, y)
    }
    pub fn hit(&self, x: u16, y: u16, session: &Session) -> Option<(usize, usize)> {
        self.lists.iter().enumerate().find_map(|(pane, rect)| {
            if !rect.contains(x, y) {
                return None;
            }
            let app = session.panes.get(pane)?.app.as_ref()?;
            let index = app.scroll + (y - rect.y) as usize;
            (index < app.entries.len()).then_some((pane, index))
        })
    }

    pub fn destination(&self, x: u16, y: u16, session: &Session) -> Option<std::path::PathBuf> {
        self.destination_pane(x, y, session).map(|(_, path)| path)
    }

    pub fn destination_pane(
        &self,
        x: u16,
        y: u16,
        session: &Session,
    ) -> Option<(usize, std::path::PathBuf)> {
        self.lists.iter().enumerate().find_map(|(pane, rect)| {
            if !rect.contains(x, y) {
                return None;
            }
            let app = session.panes.get(pane)?.app.as_ref()?;
            let index = app.scroll + (y - rect.y) as usize;
            let path = app
                .entries
                .get(index)
                .filter(|entry| entry.is_dir)
                .map(|entry| entry.path.clone())
                .unwrap_or_else(|| app.cwd.clone());
            Some((pane, path))
        })
    }

    pub fn draw(&mut self, session: &mut Session) -> Result<()> {
        let (w, h) = terminal::size()?;
        let mut out = io::stdout();
        queue!(
            out,
            Print("\x1b[?2026h"),
            cursor::MoveTo(0, 0),
            SetBackgroundColor(Color::Black),
            Clear(ClearType::All)
        )?;
        self.lists.clear();
        self.preview_rect = Rect::default();
        if w < 54 || h < 12 {
            self.kitty.delete()?;
            print_at(
                &mut out,
                1,
                1,
                WHITE,
                true,
                &truncate(
                    "option files · resize to at least 54 x 12",
                    w.saturating_sub(2) as usize,
                ),
            )?;
            queue!(out, Print("\x1b[?2026l"))?;
            out.flush()?;
            return Ok(());
        }
        let preview_width =
            if session.preview && ((!session.dual && w >= 92) || (session.dual && w >= 144)) {
                32
            } else {
                0
            };
        let available = w
            - 4
            - if preview_width > 0 {
                preview_width + 3
            } else {
                0
            };
        let count = if session.dual { 2 } else { 1 };
        let pane_width = (available - (count - 1) * 3) / count;
        let rows = h - 10;
        print_at(
            &mut out,
            2,
            1,
            WHITE,
            true,
            &format!(
                "{} {}",
                option_sdk::App::FILES.mark(),
                option_sdk::App::FILES.display_name()
            ),
        )?;
        print_right(
            &mut out,
            w - 2,
            1,
            DIM,
            if session.dual {
                "tab source · F5 copy · F6 move"
            } else {
                "local · files"
            },
        )?;
        line(&mut out, 2, 3, w - 4)?;
        for index in 0..count as usize {
            let x = 2 + index as u16 * (pane_width + 3);
            let rect = Rect {
                x,
                y: 5,
                width: pane_width,
                height: rows,
            };
            self.lists.push(rect);
            let pane = &mut session.panes[index];
            let active = index == session.active;
            let target_pane = session
                .drag
                .as_ref()
                .and_then(|drag| drag.target.as_ref())
                .is_some_and(|(pane_index, path)| *pane_index == index && path == &pane.location);
            let label = format!(
                "{} {}{}",
                if active { ">" } else { " " },
                compact_path(&pane.location, pane_width.saturating_sub(2) as usize),
                if target_pane { " · DROP" } else { "" }
            );
            print_at(
                &mut out,
                x,
                2,
                if active { WHITE } else { DIM },
                active,
                &truncate(&label, pane_width as usize),
            )?;
            print_at(&mut out, x + 4, 4, DIM, false, "NAME")?;
            print_right(&mut out, x + pane_width, 4, DIM, "SIZE")?;
            if let Some(app) = &mut pane.app {
                if app.selected < app.scroll {
                    app.scroll = app.selected;
                }
                if app.selected >= app.scroll + rows as usize {
                    app.scroll = app.selected + 1 - rows as usize;
                }
                app.scroll = app.scroll.min(app.entries.len().saturating_sub(1));
                if app.entries.is_empty() {
                    print_at(
                        &mut out,
                        x + 1,
                        5,
                        DIM,
                        false,
                        &truncate(
                            if app.filter.is_empty() {
                                "This folder is empty"
                            } else {
                                "No names match · / edit filter"
                            },
                            pane_width.saturating_sub(1) as usize,
                        ),
                    )?;
                }
                for (row, (entry_index, entry)) in app
                    .entries
                    .iter()
                    .enumerate()
                    .skip(app.scroll)
                    .take(rows as usize)
                    .enumerate()
                {
                    let selected = active && entry_index == app.selected;
                    let target = session
                        .drag
                        .as_ref()
                        .and_then(|drag| drag.target.as_ref())
                        .is_some_and(|(pane, path)| *pane == index && path == &entry.path);
                    let mark = if app.marked.contains(&entry.path) {
                        "*"
                    } else {
                        " "
                    };
                    let icon = if entry.is_dir {
                        ">"
                    } else if entry.is_symlink {
                        "@"
                    } else {
                        "·"
                    };
                    let label = format!(
                        "{}{} {icon} {}",
                        if selected { ">" } else { " " },
                        mark,
                        truncate(&entry.name, pane_width.saturating_sub(17) as usize)
                    );
                    let meta = if target {
                        "DROP".into()
                    } else if entry.is_dir {
                        "-".into()
                    } else {
                        human_size(entry.size)
                    };
                    if selected || target {
                        selected_row(&mut out, x, rect.y + row as u16, pane_width, &label, &meta)?;
                    } else {
                        print_at(&mut out, x, rect.y + row as u16, WHITE, false, &label)?;
                        print_right(&mut out, x + pane_width, rect.y + row as u16, DIM, &meta)?;
                    }
                }
            } else {
                let message = pane
                    .error
                    .as_deref()
                    .unwrap_or("Loading directory · input remains active");
                print_at(
                    &mut out,
                    x,
                    5,
                    DIM,
                    false,
                    &truncate(message, pane_width as usize),
                )?;
            }
        }
        if preview_width > 0 {
            let rect = Rect {
                x: w - 2 - preview_width,
                y: 4,
                width: preview_width,
                height: h - 9,
            };
            self.preview_rect = rect;
            let entry = session.panes[session.active]
                .app
                .as_ref()
                .and_then(|app| app.current());
            let generation =
                self.preview
                    .request_entry(entry, rect.width, rect.height.saturating_sub(2));
            if generation != self.preview_generation {
                self.preview_generation = generation;
                self.preview_text = false;
            }
            print_at(
                &mut out,
                rect.x,
                rect.y,
                DIM,
                false,
                "preview · p text/image · [ ]",
            )?;
            match self.preview.state() {
                PreviewState::Empty => {
                    print_at(&mut out, rect.x, rect.y + 2, DIM, false, "No file selected")?
                }
                PreviewState::Loading => {
                    print_at(&mut out, rect.x, rect.y + 2, DIM, false, "Loading preview")?
                }
                PreviewState::Error(error) => {
                    print_at(
                        &mut out,
                        rect.x,
                        rect.y + 2,
                        WHITE,
                        false,
                        "Preview unavailable",
                    )?;
                    for (i, text) in wrap_text(error, rect.width as usize)
                        .iter()
                        .take(rect.height.saturating_sub(4) as usize)
                        .enumerate()
                    {
                        print_at(&mut out, rect.x, rect.y + 3 + i as u16, DIM, false, text)?;
                    }
                }
                PreviewState::Ready(preview) => {
                    print_at(
                        &mut out,
                        rect.x,
                        rect.y + 1,
                        DIM,
                        false,
                        &format!(
                            "{} · line {}",
                            preview.kind,
                            self.preview.scroll_offset() + 1
                        ),
                    )?;
                    if self.preview_text || preview.image.is_none() || !self.kitty.supported() {
                        for (i, text) in self
                            .preview
                            .text_lines(rect.height.saturating_sub(2) as usize)
                            .iter()
                            .enumerate()
                        {
                            print_at(
                                &mut out,
                                rect.x,
                                rect.y + 2 + i as u16,
                                WHITE,
                                false,
                                &truncate(text, rect.width as usize),
                            )?;
                        }
                    }
                }
            }
        } else {
            self.preview.request_entry(None, 0, 0);
        }
        let footer = h - 4;
        line(&mut out, 2, footer - 1, w - 4)?;
        let state = session.panes[session.active]
            .app
            .as_ref()
            .map(|app| {
                format!(
                    "{} items · {} marked · {}{}",
                    app.entries.len(),
                    app.marked.len(),
                    app.sort.label(),
                    if app.filter.is_empty() {
                        String::new()
                    } else {
                        format!(" · /{}", app.filter)
                    }
                )
            })
            .unwrap_or_else(|| {
                if session.panes[session.active].loading {
                    "directory worker running".into()
                } else {
                    "directory unavailable · ctrl+r retry".into()
                }
            });
        let clip = session
            .clipboard
            .as_ref()
            .map(|clip| {
                format!(
                    " · {} {}",
                    if clip.mode == ClipboardMode::Copy {
                        "copy"
                    } else {
                        "cut"
                    },
                    clip.paths.len()
                )
            })
            .unwrap_or_default();
        print_at(
            &mut out,
            2,
            footer,
            DIM,
            false,
            &truncate(&format!("{state}{clip}"), (w - 4) as usize),
        )?;
        let pane = &session.panes[session.active];
        let app_status = pane.error.as_deref().unwrap_or_else(|| {
            pane.app
                .as_ref()
                .map(|app| app.status.as_str())
                .unwrap_or("")
        });
        let status = if let Some(transfer) = &session.transfer {
            transfer
                .progress
                .as_ref()
                .map(|p| {
                    format!(
                        "transfer {}/{} · {} · {} · ctrl+c cancel",
                        p.items_done,
                        p.items_total,
                        human_size(p.bytes_done),
                        p.current
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_else(|| "preparing transfer · ctrl+c cancel".into())
        } else if session.native_armed {
            "native source armed · drag a file out · D disarm".into()
        } else if session.status.is_empty() || pane.error.is_some() {
            app_status.to_string()
        } else {
            session.status.clone()
        };
        print_at(
            &mut out,
            2,
            footer + 1,
            WHITE,
            false,
            &truncate(&status, (w - 4) as usize),
        )?;
        print_at(
            &mut out,
            2,
            footer + 2,
            DIM,
            false,
            &truncate(
                "↑↓ move  enter open  t mark  / search  w panes  ? help",
                (w - 4) as usize,
            ),
        )?;
        let overlay = session.help
            || session.prompt.is_some()
            || session.show_errors
            || session
                .transfer
                .as_ref()
                .is_some_and(|t| t.conflict.is_some());
        if session.help {
            overlay_help(&mut out, w, h, session.help_offset)?;
        }
        if session.show_errors {
            overlay_lines(
                &mut out,
                w,
                h,
                "transfer errors · ↑↓ scroll · esc close",
                &session.errors,
                session.error_offset,
            )?;
        }
        if let Some(transfer) = &session.transfer {
            if let Some(conflict) = &transfer.conflict {
                let lines = vec![
                    format!("source: {}", conflict.source.display()),
                    format!("exists: {}", conflict.target.display()),
                    String::new(),
                    "o overwrite · s skip · k keep both".into(),
                    format!(
                        "a apply to all: {} · esc cancel transfer",
                        if transfer.apply_all { "on" } else { "off" }
                    ),
                ];
                overlay_lines(&mut out, w, h, "destination exists", &lines, 0)?;
            }
        }
        if let Some(prompt) = &session.prompt {
            overlay_prompt(&mut out, w, h, prompt)?;
        }
        queue!(out, Print("\x1b[?2026l"))?;
        out.flush()?;
        if overlay || self.preview_rect.width == 0 {
            self.kitty.delete()?;
        } else if let PreviewState::Ready(preview) = self.preview.state() {
            let r = self.preview_rect;
            if let Err(error) = self.kitty.render(
                if self.preview_text {
                    None
                } else {
                    preview.image.as_ref()
                },
                r.x,
                r.y + 2,
                r.width,
                r.height.saturating_sub(2),
            ) {
                session.status = format!("graphics preview: {error}");
            }
        } else {
            self.kitty.delete()?;
        }
        Ok(())
    }
}

impl Drop for TerminalUi {
    fn drop(&mut self) {
        self.kitty.shutdown();
        let _ = execute!(
            io::stdout(),
            Print("\x1b[?2026l"),
            cursor::Show,
            DisableBracketedPaste,
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
    Drop,
}
pub struct Prompt {
    pub kind: PromptKind,
    pub value: String,
    pub original: String,
}
impl Prompt {
    fn label(&self) -> &'static str {
        match self.kind {
            PromptKind::Folder => "new folder",
            PromptKind::File => "new file",
            PromptKind::Rename => "rename",
            PromptKind::Delete => "trash selection? type y",
            PromptKind::Search => "filter names · updates while typing",
            PromptKind::Drop => "drop files here? c copy · m move",
        }
    }
}

fn selected_row(
    out: &mut impl Write,
    x: u16,
    y: u16,
    width: u16,
    label: &str,
    meta: &str,
) -> Result<()> {
    let meta = truncate(meta, width.saturating_sub(2) as usize);
    let meta_len = unicode_width::UnicodeWidthStr::width(meta.as_str());
    let label = truncate(label, (width as usize).saturating_sub(meta_len + 1));
    let gap = (width as usize)
        .saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()) + meta_len);
    queue!(
        out,
        cursor::MoveTo(x, y),
        SetForegroundColor(Color::Black),
        SetBackgroundColor(WHITE),
        SetAttribute(Attribute::Bold),
        Print(format!("{label}{}{meta}", " ".repeat(gap))),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}

fn help_rows() -> Vec<String> {
    [
        "enter / → open · ← / backspace parent",
        "j/k / ↑↓ move · g/G first/last · pgup/pgdn",
        "t / insert mark · shift+↑↓ mark range",
        "ctrl+a select all · u clear marks",
        "/ / ctrl+f incremental filter · esc restore",
        "w toggle two panes · tab active source",
        "F5 copy / F6 move source to other pane",
        "c / x / v batch copy / cut / paste",
        "ctrl+c cancel transfer · ! partial errors",
        "conflict: o overwrite / s skip / k keep both",
        "conflict: a apply choice to all · esc cancel",
        "drag inside a list · choose c copy / m move",
        "ctrl+d native send pad · ctrl+v receive pad",
        "D arm/disarm terminal OSC72 source",
        "space preview · p text/image · [ / ] scroll",
        "a / . hidden · s sort · ~ home · - previous",
        "n / N new folder / file · r / F2 rename",
        "d / delete trash marked files (confirm y)",
        "e / o editor / system opener",
        "F5 (single pane) / ctrl+r refresh",
        "q / esc quit · ? close help",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn help_limit() -> usize {
    overlay_limit(&help_rows())
}

pub fn overlay_limit(rows: &[String]) -> usize {
    let (w, h) = terminal::size().unwrap_or((54, 12));
    let width = w.saturating_sub(4).min(84).saturating_sub(4) as usize;
    let height = h
        .saturating_sub(2)
        .min(rows.len().saturating_add(4).min(u16::MAX as usize) as u16)
        .max(6);
    rows.iter()
        .map(|line| wrap_text(line, width).len())
        .sum::<usize>()
        .saturating_sub(height.saturating_sub(3) as usize)
}

fn overlay_help(out: &mut impl Write, w: u16, h: u16, offset: usize) -> Result<()> {
    overlay_lines(
        out,
        w,
        h,
        "keyboard · ↑↓ scroll help · ? close",
        &help_rows(),
        offset,
    )
}

fn overlay_lines(
    out: &mut impl Write,
    w: u16,
    h: u16,
    title: &str,
    rows: &[String],
    offset: usize,
) -> Result<()> {
    let width = (w - 4).min(84);
    let height = (h - 2)
        .min(rows.len().saturating_add(4).min(u16::MAX as usize) as u16)
        .max(6);
    let x = (w - width) / 2;
    let y = (h - height) / 2;
    box_fill(out, x, y, width, height)?;
    print_at(
        out,
        x + 2,
        y + 1,
        WHITE,
        true,
        &truncate(title, width.saturating_sub(4) as usize),
    )?;
    let wrapped: Vec<String> = rows
        .iter()
        .flat_map(|line| wrap_text(line, width.saturating_sub(4) as usize))
        .collect();
    let offset = offset.min(
        wrapped
            .len()
            .saturating_sub(height.saturating_sub(3) as usize),
    );
    for (i, row) in wrapped
        .iter()
        .skip(offset)
        .take(height.saturating_sub(3) as usize)
        .enumerate()
    {
        print_at(out, x + 2, y + 3 + i as u16, WHITE, false, row)?;
    }
    Ok(())
}

fn overlay_prompt(out: &mut impl Write, w: u16, h: u16, prompt: &Prompt) -> Result<()> {
    let width = 64.min(w - 4);
    let x = (w - width) / 2;
    let y = h / 2 - 3;
    box_fill(out, x, y, width, 7)?;
    print_at(
        out,
        x + 2,
        y + 1,
        DIM,
        false,
        &truncate(prompt.label(), width.saturating_sub(4) as usize),
    )?;
    print_at(
        out,
        x + 2,
        y + 3,
        WHITE,
        true,
        &format!(
            "> {}",
            truncate(&prompt.value, width.saturating_sub(6) as usize)
        ),
    )?;
    print_at(out, x + 2, y + 5, DIM, false, "enter confirm · esc cancel")
}

fn box_fill(out: &mut impl Write, x: u16, y: u16, w: u16, h: u16) -> Result<()> {
    for row in 0..h {
        queue!(
            out,
            cursor::MoveTo(x, y + row),
            SetBackgroundColor(Color::Black),
            Print(" ".repeat(w as usize)),
            ResetColor
        )?;
    }
    Ok(())
}
fn line(out: &mut impl Write, x: u16, y: u16, width: u16) -> Result<()> {
    print_at(out, x, y, DIM, false, &"─".repeat(width as usize))
}
fn safe_text(text: &str) -> std::borrow::Cow<'_, str> {
    if text.chars().any(char::is_control) {
        std::borrow::Cow::Owned(crate::preview::sanitize(text).join(" "))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

fn print_at(
    out: &mut impl Write,
    x: u16,
    y: u16,
    color: Color,
    bold: bool,
    text: &str,
) -> Result<()> {
    let safe = safe_text(text);
    queue!(
        out,
        cursor::MoveTo(x, y),
        SetForegroundColor(color),
        SetBackgroundColor(Color::Black),
        SetAttribute(if bold {
            Attribute::Bold
        } else {
            Attribute::NormalIntensity
        }),
        Print(safe.as_ref()),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}
fn print_right(out: &mut impl Write, right: u16, y: u16, color: Color, text: &str) -> Result<()> {
    print_at(
        out,
        right.saturating_sub(unicode_width::UnicodeWidthStr::width(text) as u16),
        y,
        color,
        false,
        text,
    )
}
fn truncate(s: &str, max: usize) -> String {
    let safe = safe_text(s);
    let s = safe.as_ref();
    if max == 0 {
        return String::new();
    }
    if unicode_width::UnicodeWidthStr::width(s) <= max {
        return s.into();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw + 1 > max {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}
fn compact_path(path: &Path, max: usize) -> String {
    let full = path.display().to_string();
    let full = safe_text(&full);
    if unicode_width::UnicodeWidthStr::width(full.as_ref()) <= max {
        return full.into_owned();
    }
    if max <= 2 {
        return truncate(&full, max);
    }
    let tail_budget = ((max - 1) * 2 / 3).max(1);
    let head_budget = max - 1 - tail_budget;
    let mut head = String::new();
    let mut used = 0;
    for c in full.chars() {
        let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + width > head_budget {
            break;
        }
        head.push(c);
        used += width;
    }
    let mut tail = String::new();
    used = 0;
    for c in full.chars().rev() {
        let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + width > tail_budget {
            break;
        }
        tail.push(c);
        used += width;
    }
    format!("{head}…{}", tail.chars().rev().collect::<String>())
}
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > width.max(1) {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push(c);
        used += cw;
    }
    lines.push(line);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rectangles_exclude_headers_footers_and_other_columns() {
        let rect = Rect {
            x: 2,
            y: 5,
            width: 30,
            height: 10,
        };
        assert!(rect.contains(2, 5));
        assert!(rect.contains(31, 14));
        assert!(!rect.contains(1, 5));
        assert!(!rect.contains(32, 5));
        assert!(!rect.contains(2, 4));
        assert!(!rect.contains(2, 15));
    }
    #[test]
    fn truncate_counts_visual_width() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 9), "hello wo…");
        assert_eq!(truncate("日本語のテキスト", 6), "日本…");
        assert_eq!(truncate("anything", 0), "");
    }
    #[test]
    fn compact_path_elides_middle() {
        let p = Path::new("/home/firefly/Projects/optionFiles");
        let out = compact_path(p, 20);
        assert!(out.ends_with("optionFiles"));
        assert!(unicode_width::UnicodeWidthStr::width(out.as_str()) <= 20);
    }
    #[test]
    fn all_row_text_removes_terminal_control_sequences_before_measuring() {
        assert_eq!(truncate("a\x1b[31mb\x1b[0m", 10), "ab");
        assert_eq!(truncate("a\x1b]52;c;cQ==\x07b", 10), "ab");
        assert_eq!(truncate("a\x1b_Gq;payload\x1b\\b", 10), "ab");
        let mut out = Vec::new();
        selected_row(&mut out, 0, 0, 20, "a\x1b]52;c;cQ==\x07b", "1").unwrap();
        let encoded = String::from_utf8(out).unwrap();
        assert!(!encoded.contains("52;c"));
        assert!(!encoded.contains('\x07'));
    }
}
