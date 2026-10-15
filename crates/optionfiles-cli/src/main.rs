mod cli;
mod dnd;
mod input;
mod interaction;
mod kitty;
mod preview;
mod ui;

use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command};
use optionfiles_core::{App, Entry, SortMode, format_time, human_size, read_dir};

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
    let clipboard = has_binary("wl-copy") || has_binary("xclip");
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
        "  clipboard (wl-copy/xclip): {}",
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
        "  magick (image thumbnails): {}",
        if magick {
            "ok"
        } else {
            "missing (image thumbnails unavailable)"
        }
    );
    Ok(())
}

fn interactive(path: &Path, all: bool) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return list(path, all);
    }
    interaction::run(path, all)
}

fn open_external(path: &Path, app: &mut App) {
    let command = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match ProcessCommand::new(command)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
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
    match command.status() {
        Ok(status) if status.success() => {
            app.status = format!(
                "edited {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        }
        Ok(status) => app.status = format!("editor exited with {status}"),
        Err(e) => app.status = e.to_string(),
    }
}
