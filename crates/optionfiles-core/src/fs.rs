use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Name,
    Size,
    Modified,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Size,
            Self::Size => Self::Modified,
            Self::Modified => Self::Name,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Modified => "date",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub normalized_name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Entry {
    pub fn load(path: PathBuf) -> Result<Self> {
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            normalized_name: name.to_lowercase(),
            name,
            path,
            is_dir: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

pub fn read_dir(path: &Path, hidden: bool, sort: SortMode) -> Result<Vec<Entry>> {
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("cannot open {}", path.display()))?
        .filter_map(Result::ok)
        .filter(|e| hidden || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| Entry::load(e.path()).ok())
        .collect::<Vec<_>>();
    sort_entries(&mut entries, sort);
    Ok(entries)
}

pub fn sort_entries(entries: &mut [Entry], sort: SortMode) {
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        let ord = match sort {
            SortMode::Name => a.normalized_name.cmp(&b.normalized_name),
            SortMode::Size => a.size.cmp(&b.size),
            SortMode::Modified => a.modified.cmp(&b.modified),
        };
        if ord == Ordering::Equal {
            a.name.cmp(&b.name)
        } else {
            ord
        }
    });
}

pub fn copy_recursively(from: &Path, to: &Path) -> Result<()> {
    crate::transfer::copy_to_path(from, to)
}

pub fn human_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
