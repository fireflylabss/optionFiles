mod app;
mod cli;
mod fs;
mod kitty;
mod ui;

use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result};
use app::{App, ClipboardMode};
use clap::Parser;
use cli::{Cli, Command};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
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
    let cli = Cli::parse();
    let all = cli.all;
    match cli.command {
        Some(Command::List { path }) => list(&path, all),
        Some(Command::Info { path }) => info(&path),
        Some(Command::Open { path }) => interactive(&path, all),
        None => interactive(cli.path.as_deref().unwrap_or(Path::new(".")), all),
    }
}

fn list(path: &Path, all: bool) -> Result<()> {
    for entry in fs::read_dir(path, all, fs::SortMode::Name)? {
        println!(
            "{}  {:>10}  {}",
            if entry.is_dir { "d" } else { "·" },
            if entry.is_dir {
                "—".into()
            } else {
                fs::human_size(entry.size)
            },
            entry.name
        );
    }
    Ok(())
}

fn info(path: &Path) -> Result<()> {
    let entry = fs::Entry::load(path.to_path_buf())?;
    println!("◆ {}", entry.name);
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
    println!("  size      {}", fs::human_size(entry.size));
    println!(
        "  path      {}",
        entry.path.canonicalize().unwrap_or(entry.path).display()
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
                    KeyCode::Char('a') => {
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
                    KeyCode::Char('r') => {
                        if let Some(name) = app.current().map(|e| e.name.clone()) {
                            prompt = Some(Prompt {
                                kind: PromptKind::Rename,
                                value: name,
                            });
                        }
                    }
                    KeyCode::Char('d') => {
                        prompt = Some(Prompt {
                            kind: PromptKind::Delete,
                            value: String::new(),
                        })
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
