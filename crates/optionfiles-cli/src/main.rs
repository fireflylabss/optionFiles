mod cli;
mod kitty;
mod ui;

use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use optionfiles_core::{App, ClipboardMode, Entry, SortMode, format_time, human_size, read_dir};
use ui::{Prompt, PromptKind, TerminalUi};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        for cause in error.chain().skip(1) {
            eprintln!("  ↳ {cause}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    option_sdk::App::FILES
        .ensure()
        .context("failed to ensure ~/.option/files")?;
    let cli = Cli::parse();
    let all = cli.all;
    match cli.command {
        Some(Command::List { path }) => list(&path, all),
        Some(Command::Info { path }) => info(&path),
        Some(Command::Tree { path, depth }) => tree(&path, all, depth),
        Some(Command::Doctor { json }) => doctor(json),
        Some(Command::Open { path }) => interactive(&path, all),
        None => interactive(cli.path.as_deref().unwrap_or(Path::new(".")), all),
    }
}

fn tree(path: &Path, all: bool, depth: u8) -> Result<()> {
    let root = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    println!("{} {}", option_sdk::App::FILES.mark(), root.display());
    print_tree(&root, all, depth as usize, "")
}

fn print_tree(path: &Path, all: bool, depth: usize, prefix: &str) -> Result<()> {
    if depth == 0 || !path.is_dir() {
        return Ok(());
    }
    let entries = read_dir(path, all, SortMode::Name)?;
    let len = entries.len();
    for (index, entry) in entries.into_iter().enumerate() {
        let last = index + 1 == len;
        println!(
            "{prefix}{} {}{}",
            if last { "└─" } else { "├─" },
            entry.name,
            if entry.is_dir { "/" } else { "" }
        );
        if entry.is_dir {
            let next = format!("{prefix}{} ", if last { "  " } else { "│ " });
            print_tree(&entry.path, all, depth - 1, &next)?;
        }
    }
    Ok(())
}

fn list(path: &Path, all: bool) -> Result<()> {
    for entry in read_dir(path, all, SortMode::Name)? {
        println!(
            "{}  {:>10}  {}",
            if entry.is_dir { "d" } else { "·" },
            if entry.is_dir {
                "—".into()
            } else {
                human_size(entry.size)
            },
            entry.name
        );
    }
    Ok(())
}

fn info(path: &Path) -> Result<()> {
    let entry = Entry::load(path.to_path_buf())?;
    println!("{} {}", option_sdk::App::FILES.mark(), entry.name);
    println!(
        "  type      {}",
        if entry.is_dir {
            "directory"
        } else if entry.is_symlink {
            "symbolic link"
        } else {
            "file"
        }
    );
    println!("  size      {}", human_size(entry.size));
    println!(
        "  path      {}",
        entry.path.canonicalize().unwrap_or(entry.path).display()
    );
    if let Ok(metadata) = path.symlink_metadata() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            'd'
        } else if file_type.is_symlink() {
            'l'
        } else {
            '-'
        };
        let perms = format!(
            "{}{}{}{}{}{}{}{}{}",
            if mode & 0o400 != 0 { 'r' } else { '-' },
            if mode & 0o200 != 0 { 'w' } else { '-' },
            if mode & 0o100 != 0 { 'x' } else { '-' },
            if mode & 0o040 != 0 { 'r' } else { '-' },
            if mode & 0o020 != 0 { 'w' } else { '-' },
            if mode & 0o010 != 0 { 'x' } else { '-' },
            if mode & 0o004 != 0 { 'r' } else { '-' },
            if mode & 0o002 != 0 { 'w' } else { '-' },
            if mode & 0o001 != 0 { 'x' } else { '-' },
        );
        println!("  mode      {kind}{perms}");
        println!(
            "  owner     uid {} · gid {}",
            metadata.uid(),
            metadata.gid()
        );
    }
    if let Some(modified) = entry.modified {
        println!("  modified  {}", format_time(modified));
    }
    Ok(())
}

fn has_binary(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        if dir.join(name).is_file() {
            return true;
        }
    }
    false
}

fn doctor(json: bool) -> Result<()> {
    let dir = option_sdk::App::FILES.dir();
    let dir_ok = dir.is_dir();
    let opener = if cfg!(target_os = "macos") {
        has_binary("open")
    } else {
        has_binary("xdg-open")
    };
    let clipboard = has_binary("wl-copy") || has_binary("xclip") || has_binary("pbcopy");
    let trash = has_binary("gio") || has_binary("trash");
    let magick = has_binary("magick") || has_binary("convert");
    if json {
        println!(
            "{{\"state_dir\":\"{}\",\"state_ok\":{dir_ok},\"opener\":{opener},\"clipboard\":{clipboard},\"trash\":{trash},\"magick\":{magick}}}",
            dir.display(),
        );
        return Ok(());
    }
    println!("{} doctor", option_sdk::App::FILES.mark());
    println!(
        "  state dir: {} ({})",
        dir.display(),
        if dir_ok { "ok" } else { "missing" }
    );
    println!(
        "  opener (xdg-open/open): {}",
        if opener { "ok" } else { "missing" }
    );
    println!(
        "  clipboard (wl-copy/xclip/pbcopy): {}",
        if clipboard {
            "ok (system)"
        } else {
            "fallback interno"
        }
    );
    println!(
        "  trash (gio/trash): {}",
        if trash { "ok (system)" } else { "fallback XDG" }
    );
    println!(
        "  magick (previews não-PNG): {}",
        if magick {
            "ok"
        } else {
            "ausente — só PNG nativo"
        }
    );
    Ok(())
}

fn interactive(path: &Path, all: bool) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return list(path, all);
    }
    let mut app = App::new(path, all)?;
    let mut terminal = TerminalUi::enter()?;
    let mut prompt: Option<Prompt> = None;
    loop {
        terminal.draw(&mut app, prompt.as_ref())?;
        match event::read()? {
            Event::Resize(_, _) => continue,
            Event::Mouse(mouse) if prompt.is_none() && !app.help => match mouse.kind {
                MouseEventKind::ScrollUp => app.move_by(-3),
                MouseEventKind::ScrollDown => app.move_by(3),
                MouseEventKind::Down(_) => {
                    let row = mouse.row.saturating_sub(5) as usize + app.scroll;
                    if row < app.entries.len() {
                        app.selected = row;
                    }
                }
                _ => {}
            },
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(p) = prompt.as_mut() {
                    match key.code {
                        KeyCode::Esc => prompt = None,
                        KeyCode::Enter => {
                            let p = prompt.take().unwrap();
                            let result = match p.kind {
                                PromptKind::Folder => app.create_dir(p.value.trim()),
                                PromptKind::File => app.create_file(p.value.trim()),
                                PromptKind::Rename => app.rename_current(p.value.trim()),
                                PromptKind::Delete if p.value.eq_ignore_ascii_case("y") => {
                                    app.delete_current()
                                }
                                PromptKind::Delete => {
                                    app.status = "delete cancelled".into();
                                    Ok(())
                                }
                                PromptKind::Search => app.set_filter(p.value),
                            };
                            if let Err(e) = result {
                                app.status = e.to_string();
                            }
                        }
                        KeyCode::Backspace => {
                            p.value.pop();
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            p.value.push(c)
                        }
                        _ => {}
                    }
                    continue;
                }
                if app.help {
                    if matches!(
                        key.code,
                        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
                    ) {
                        app.help = false;
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        prompt = Some(Prompt {
                            kind: PromptKind::Search,
                            value: app.filter.clone(),
                        })
                    }
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(e) = app.refresh() {
                            app.status = e.to_string();
                        } else {
                            app.status = "refreshed".into();
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => app.move_by(-1),
                    KeyCode::Down | KeyCode::Char('j') => app.move_by(1),
                    KeyCode::PageUp => app.move_by(-10),
                    KeyCode::PageDown => app.move_by(10),
                    KeyCode::Home | KeyCode::Char('g') => app.home(),
                    KeyCode::End | KeyCode::Char('G') => app.end(),
                    KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                        if let Err(e) = app.parent() {
                            app.status = e.to_string();
                        }
                    }
                    KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => match app.enter() {
                        Ok(Some(path)) => open_external(&path, &mut app),
                        Ok(None) => {}
                        Err(e) => app.status = e.to_string(),
                    },
                    KeyCode::Char('o') => {
                        if let Some(path) = app.current().map(|e| e.path.clone()) {
                            open_external(&path, &mut app);
                        }
                    }
                    KeyCode::Char('e') => {
                        if let Some(path) = app.current().map(|e| e.path.clone()) {
                            edit_with_editor(&path, &mut app);
                        }
                    }
                    KeyCode::Char('a') | KeyCode::Char('.') => {
                        if let Err(e) = app.toggle_hidden() {
                            app.status = e.to_string();
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Err(e) = app.cycle_sort() {
                            app.status = e.to_string();
                        }
                    }
                    KeyCode::Char(' ') => app.preview = !app.preview,
                    KeyCode::Char('/') => {
                        prompt = Some(Prompt {
                            kind: PromptKind::Search,
                            value: app.filter.clone(),
                        })
                    }
                    KeyCode::Char('~') => {
                        if let Err(e) = app.go_home() {
                            app.status = e.to_string();
                        }
                    }
                    KeyCode::Char('-') => {
                        if let Err(e) = app.go_previous() {
                            app.status = e.to_string();
                        }
                    }
                    KeyCode::Char('?') => app.help = true,
                    KeyCode::Char('c') => app.set_clipboard(ClipboardMode::Copy),
                    KeyCode::Char('x') => app.set_clipboard(ClipboardMode::Cut),
                    KeyCode::Char('v') => {
                        if let Err(e) = app.paste() {
                            app.status = e.to_string();
                        }
                    }
                    KeyCode::Char('n') => {
                        prompt = Some(Prompt {
                            kind: PromptKind::Folder,
                            value: String::new(),
                        })
                    }
                    KeyCode::Char('N') => {
                        prompt = Some(Prompt {
                            kind: PromptKind::File,
                            value: String::new(),
                        })
                    }
                    KeyCode::Char('r') | KeyCode::F(2) => {
                        if let Some(name) = app.current().map(|e| e.name.clone()) {
                            prompt = Some(Prompt {
                                kind: PromptKind::Rename,
                                value: name,
                            });
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Delete => {
                        prompt = Some(Prompt {
                            kind: PromptKind::Delete,
                            value: String::new(),
                        })
                    }
                    KeyCode::F(5) => {
                        if let Err(e) = app.refresh() {
                            app.status = e.to_string();
                        } else {
                            app.status = "refreshed".into();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn open_external(path: &Path, app: &mut App) {
    let command = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match ProcessCommand::new(command)
        .arg(path)
        .spawn()
        .with_context(|| format!("cannot run {command}"))
    {
        Ok(_) => {
            app.status = format!(
                "opened {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        }
        Err(e) => app.status = e.to_string(),
    }
}

fn edit_with_editor(path: &Path, app: &mut App) {
    let editor = std::env::var("EDITOR").or_else(|_| std::env::var("VISUAL"));
    let editor = match editor {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => {
            app.status = "set $EDITOR to edit files".into();
            return;
        }
    };
    let mut command = if cfg!(target_os = "windows") {
        ProcessCommand::new("cmd")
    } else {
        ProcessCommand::new("sh")
    };
    command
        .arg(if cfg!(target_os = "windows") {
            "/C"
        } else {
            "-c"
        })
        .arg(format!("{editor} \"$1\""))
        .arg("optionfiles-editor")
        .arg(path);
    match command.spawn() {
        Ok(_) => {
            app.status = format!(
                "editing {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        }
        Err(e) => app.status = e.to_string(),
    }
}
