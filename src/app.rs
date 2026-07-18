use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::fs::{Entry, SortMode, copy_recursively, read_dir};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Debug, Clone)]
pub struct Clipboard {
    pub path: PathBuf,
    pub mode: ClipboardMode,
}

pub struct App {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub scroll: usize,
    pub show_hidden: bool,
    pub sort: SortMode,
    pub clipboard: Option<Clipboard>,
    pub status: String,
    pub help: bool,
    pub preview: bool,
}

impl App {
    pub fn new(path: &Path, show_hidden: bool) -> Result<Self> {
        let cwd = if path.is_dir() {
            path.canonicalize()?
        } else {
            path.parent().unwrap_or(Path::new(".")).canonicalize()?
        };
        let mut app = Self {
            cwd,
            entries: vec![],
            selected: 0,
            scroll: 0,
            show_hidden,
            sort: SortMode::Name,
            clipboard: None,
            status: String::new(),
            help: false,
            preview: true,
        };
        app.refresh()?;
        if path.is_file() {
            if let Some(name) = path.file_name() {
                app.selected = app
                    .entries
                    .iter()
                    .position(|e| e.path.file_name() == Some(name))
                    .unwrap_or(0);
            }
        }
        Ok(app)
    }

    pub fn refresh(&mut self) -> Result<()> {
        let selected = self.current().map(|e| e.name.clone());
        self.entries = read_dir(&self.cwd, self.show_hidden, self.sort)?;
        self.selected = selected
            .and_then(|n| self.entries.iter().position(|e| e.name == n))
            .unwrap_or(self.selected.min(self.entries.len().saturating_sub(1)));
        Ok(())
    }
    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }
    pub fn move_by(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.entries.len() - 1);
    }
    pub fn home(&mut self) {
        self.selected = 0;
    }
    pub fn end(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }
    pub fn enter(&mut self) -> Result<Option<PathBuf>> {
        let Some(entry) = self.current().cloned() else {
            return Ok(None);
        };
        if entry.is_dir {
            self.cwd = entry.path.canonicalize()?;
            self.selected = 0;
            self.scroll = 0;
            self.refresh()?;
            Ok(None)
        } else {
            Ok(Some(entry.path))
        }
    }
    pub fn parent(&mut self) -> Result<()> {
        let old = self.cwd.clone();
        let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
            return Ok(());
        };
        self.cwd = parent;
        self.selected = 0;
        self.scroll = 0;
        self.refresh()?;
        if let Some(name) = old.file_name() {
            self.selected = self
                .entries
                .iter()
                .position(|e| e.path.file_name() == Some(name))
                .unwrap_or(0);
        }
        Ok(())
    }
    pub fn toggle_hidden(&mut self) -> Result<()> {
        self.show_hidden = !self.show_hidden;
        self.refresh()
    }
    pub fn cycle_sort(&mut self) -> Result<()> {
        self.sort = self.sort.next();
        self.refresh()
    }
    pub fn set_clipboard(&mut self, mode: ClipboardMode) {
        if let Some(entry) = self.current() {
            self.clipboard = Some(Clipboard {
                path: entry.path.clone(),
                mode,
            });
            self.status = match mode {
                ClipboardMode::Copy => "copied",
                ClipboardMode::Cut => "cut",
            }
            .into();
        }
    }
    pub fn paste(&mut self) -> Result<()> {
        let Some(clip) = self.clipboard.clone() else {
            self.status = "clipboard empty".into();
            return Ok(());
        };
        let name = clip.path.file_name().context("invalid clipboard path")?;
        let mut target = self.cwd.join(name);
        if target == clip.path && clip.mode == ClipboardMode::Cut {
            self.status = "already here".into();
            return Ok(());
        }
        if target.exists() {
            target = unique_path(&target);
        }
        match clip.mode {
            ClipboardMode::Copy => copy_recursively(&clip.path, &target)?,
            ClipboardMode::Cut => {
                fs::rename(&clip.path, &target).or_else(|_| {
                    copy_recursively(&clip.path, &target)?;
                    remove_path(&clip.path)
                })?;
                self.clipboard = None;
            }
        }
        self.status = format!(
            "pasted {}",
            target.file_name().unwrap_or_default().to_string_lossy()
        );
        self.refresh()
    }
    pub fn create_dir(&mut self, name: &str) -> Result<()> {
        fs::create_dir(self.cwd.join(name))?;
        self.status = format!("created {name}/");
        self.refresh()
    }
    pub fn create_file(&mut self, name: &str) -> Result<()> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.cwd.join(name))?;
        self.status = format!("created {name}");
        self.refresh()
    }
    pub fn rename_current(&mut self, name: &str) -> Result<()> {
        let current = self.current().context("nothing selected")?.path.clone();
        fs::rename(&current, self.cwd.join(name))?;
        self.status = format!("renamed to {name}");
        self.refresh()
    }
    pub fn delete_current(&mut self) -> Result<()> {
        let entry = self.current().context("nothing selected")?.clone();
        remove_path(&entry.path)?;
        self.status = format!("deleted {}", entry.name);
        self.refresh()
    }
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?
    } else {
        fs::remove_file(path)?
    };
    Ok(())
}

fn unique_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("copy");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1.. {
        let name = match ext {
            Some(ext) => format!("{stem} copy {n}.{ext}"),
            None => format!("{stem} copy {n}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sandbox() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("optionfiles-test-{id}"));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn creates_renames_and_copies_entries() {
        let root = sandbox();
        let mut app = App::new(&root, false).unwrap();
        app.create_file("draft.txt").unwrap();
        app.selected = app
            .entries
            .iter()
            .position(|e| e.name == "draft.txt")
            .unwrap();
        app.rename_current("notes.txt").unwrap();
        app.selected = app
            .entries
            .iter()
            .position(|e| e.name == "notes.txt")
            .unwrap();
        app.set_clipboard(ClipboardMode::Copy);
        app.paste().unwrap();
        assert!(root.join("notes.txt").exists());
        assert!(root.join("notes copy 1.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
