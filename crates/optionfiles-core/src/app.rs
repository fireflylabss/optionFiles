use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};

use crate::clipboard::{Clipboard, ClipboardMode, clipboard_export, clipboard_import};
use crate::fs::{Entry, SortMode, copy_recursively, read_dir, sort_entries};
use crate::transfer::{
    ConflictAction, ConflictResolution, TransferReport, TransferRequest, run_transfer,
};

#[derive(Clone, Default)]
struct DirectoryState {
    focused: Option<PathBuf>,
    scroll: usize,
    marked: BTreeSet<PathBuf>,
    filter: String,
}

pub struct App {
    pub cwd: PathBuf,
    pub previous_dir: Option<PathBuf>,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub scroll: usize,
    pub marked: BTreeSet<PathBuf>,
    pub show_hidden: bool,
    pub sort: SortMode,
    pub clipboard: Option<Clipboard>,
    pub status: String,
    pub help: bool,
    pub preview: bool,
    pub filter: String,
    snapshot: Vec<Entry>,
    directories: HashMap<PathBuf, DirectoryState>,
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
            previous_dir: None,
            entries: vec![],
            selected: 0,
            scroll: 0,
            marked: BTreeSet::new(),
            show_hidden,
            sort: SortMode::Name,
            clipboard: None,
            status: String::new(),
            help: false,
            preview: true,
            filter: String::new(),
            snapshot: vec![],
            directories: HashMap::new(),
        };
        app.refresh()?;
        if path.is_file() {
            if let Some(name) = path.file_name() {
                app.selected = app
                    .entries
                    .iter()
                    .position(|entry| entry.path.file_name() == Some(name))
                    .unwrap_or(0);
            }
        }
        Ok(app)
    }

    pub fn refresh(&mut self) -> Result<()> {
        let snapshot = read_dir(&self.cwd, true, self.sort)?;
        self.marked
            .retain(|path| snapshot.iter().any(|entry| &entry.path == path));
        self.snapshot = snapshot;
        self.rebuild_view();
        Ok(())
    }

    fn rebuild_view(&mut self) {
        let focused = self.current().map(|entry| entry.path.clone());
        let needle = self.filter.to_lowercase();
        let terms: Vec<&str> = needle.split_whitespace().collect();
        let mut scored = Vec::with_capacity(self.snapshot.len());
        for (index, entry) in self.snapshot.iter().enumerate() {
            if !self.show_hidden && entry.name.starts_with('.') {
                continue;
            }
            let score = if needle.is_empty() {
                Some(0)
            } else if terms.len() == 1 {
                entry.normalized_name.contains(&needle).then_some(0)
            } else {
                fuzzy_score(&entry.normalized_name, &terms)
            };
            if let Some(score) = score {
                scored.push((score, index));
            }
        }
        if terms.len() > 1 {
            scored.sort_by(|a, b| b.0.cmp(&a.0));
        }
        self.entries.clear();
        self.entries.extend(
            scored
                .into_iter()
                .map(|(_, index)| self.snapshot[index].clone()),
        );
        self.selected = focused
            .and_then(|path| self.entries.iter().position(|entry| entry.path == path))
            .unwrap_or(self.selected.min(self.entries.len().saturating_sub(1)));
        self.scroll = self.scroll.min(self.entries.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        if self.marked.is_empty() {
            self.current()
                .map(|entry| vec![entry.path.clone()])
                .unwrap_or_default()
        } else {
            self.marked.iter().cloned().collect()
        }
    }

    pub fn toggle_selection(&mut self) {
        if let Some(path) = self.current().map(|entry| entry.path.clone()) {
            if !self.marked.remove(&path) {
                self.marked.insert(path);
            }
        }
    }

    pub fn select_all(&mut self) {
        self.marked
            .extend(self.entries.iter().map(|entry| entry.path.clone()));
    }

    pub fn clear_selection(&mut self) {
        self.marked.clear();
    }

    pub fn move_by(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }

    pub fn home(&mut self) {
        self.selected = 0;
    }

    pub fn end(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }

    pub fn navigate(&mut self, path: &Path) -> Result<()> {
        let target = path
            .canonicalize()
            .with_context(|| format!("cannot open {}", path.display()))?;
        if !target.is_dir() {
            bail!("not a directory: {}", target.display());
        }
        if target == self.cwd {
            return self.refresh();
        }
        // Load first so a failed navigation leaves the active location intact.
        let snapshot = read_dir(&target, true, self.sort)?;
        self.directories.insert(
            self.cwd.clone(),
            DirectoryState {
                focused: self.current().map(|entry| entry.path.clone()),
                scroll: self.scroll,
                marked: self.marked.clone(),
                filter: self.filter.clone(),
            },
        );
        let state = self.directories.get(&target).cloned().unwrap_or_default();
        self.previous_dir = Some(std::mem::replace(&mut self.cwd, target));
        self.entries.clear();
        self.snapshot = snapshot;
        self.filter = state.filter;
        self.marked = state.marked;
        self.marked
            .retain(|path| self.snapshot.iter().any(|entry| &entry.path == path));
        self.selected = 0;
        self.scroll = state.scroll;
        self.rebuild_view();
        if let Some(path) = state.focused {
            self.selected = self
                .entries
                .iter()
                .position(|entry| entry.path == path)
                .unwrap_or(0);
        }
        Ok(())
    }

    pub fn enter(&mut self) -> Result<Option<PathBuf>> {
        let Some(entry) = self.current().cloned() else {
            return Ok(None);
        };
        if entry.is_dir || (entry.is_symlink && entry.path.is_dir()) {
            self.navigate(&entry.path)?;
            Ok(None)
        } else {
            Ok(Some(entry.path))
        }
    }

    pub fn parent(&mut self) -> Result<()> {
        let old = self.cwd.clone();
        let Some(parent) = old.parent() else {
            return Ok(());
        };
        let restored = self.directories.contains_key(parent);
        self.navigate(parent)?;
        if !restored {
            self.selected = self
                .entries
                .iter()
                .position(|entry| entry.path == old)
                .unwrap_or(0);
        }
        Ok(())
    }

    pub fn toggle_hidden(&mut self) -> Result<()> {
        self.show_hidden = !self.show_hidden;
        self.rebuild_view();
        Ok(())
    }

    pub fn cycle_sort(&mut self) -> Result<()> {
        self.sort = self.sort.next();
        sort_entries(&mut self.snapshot, self.sort);
        self.rebuild_view();
        Ok(())
    }

    pub fn set_filter(&mut self, filter: String) -> Result<()> {
        self.filter = filter;
        self.scroll = 0;
        self.rebuild_view();
        Ok(())
    }

    pub fn go_home(&mut self) -> Result<()> {
        self.navigate(&option_sdk::home_dir())
    }

    pub fn go_previous(&mut self) -> Result<()> {
        let Some(previous) = self.previous_dir.clone() else {
            self.status = "no previous location".into();
            return Ok(());
        };
        self.navigate(&previous)
    }

    pub fn set_clipboard(&mut self, mode: ClipboardMode) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let clipboard = Clipboard { paths, mode };
        let label = if mode == ClipboardMode::Copy {
            "copied"
        } else {
            "cut"
        };
        let count = clipboard.paths.len();
        self.status = match clipboard_export(&clipboard) {
            Ok(Some(helper)) => format!("{label} {count} ({helper})"),
            _ => format!("{label} {count}"),
        };
        self.clipboard = Some(clipboard);
    }

    pub fn paste(&mut self) -> Result<()> {
        let Some(clipboard) = clipboard_import().or_else(|| self.clipboard.clone()) else {
            self.status = "clipboard empty".into();
            return Ok(());
        };
        let request = TransferRequest {
            sources: clipboard.paths,
            destination: self.cwd.clone(),
            mode: clipboard.mode,
        };
        let report = run_transfer(
            &request,
            &AtomicBool::new(false),
            |_| {},
            |_| ConflictResolution {
                action: ConflictAction::KeepBoth,
                apply_to_batch: true,
            },
        );
        self.apply_transfer_report(&request, &report);
        self.refresh()
    }

    pub fn apply_transfer_report(&mut self, request: &TransferRequest, report: &TransferReport) {
        if request.mode == ClipboardMode::Cut {
            if let Some(clipboard) = self.clipboard.as_mut() {
                clipboard
                    .paths
                    .retain(|path| !report.completed.contains(path));
                if clipboard.paths.is_empty() {
                    self.clipboard = None;
                }
            }
        }
        self.status = format!(
            "{} transferred · {} skipped · {} failed{}",
            report.completed.len(),
            report.skipped.len(),
            report.errors.len(),
            if report.cancelled {
                " · cancelled"
            } else {
                ""
            }
        );
        if let Some(error) = report.errors.first() {
            self.status.push_str(&format!(" · {}", error.message));
        }
    }
    pub fn create_dir(&mut self, name: &str) -> Result<()> {
        validate_entry_name(name)?;
        fs::create_dir(self.cwd.join(name))?;
        self.status = format!("created {name}/");
        self.refresh()
    }
    pub fn create_file(&mut self, name: &str) -> Result<()> {
        validate_entry_name(name)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.cwd.join(name))?;
        self.status = format!("created {name}");
        self.refresh()
    }
    pub fn rename_current(&mut self, name: &str) -> Result<()> {
        validate_entry_name(name)?;
        let current = self.current().context("nothing selected")?.path.clone();
        fs::rename(&current, self.cwd.join(name))?;
        self.status = format!("renamed to {name}");
        self.refresh()
    }
    pub fn delete_selected(&mut self) -> Result<()> {
        let paths = self.selected_paths();
        if paths.is_empty() {
            bail!("nothing selected");
        }
        let mut completed = 0;
        let mut errors = Vec::new();
        for path in paths {
            match trash_path(&path) {
                Ok(()) => {
                    self.marked.remove(&path);
                    completed += 1;
                }
                Err(error) => errors.push(format!("{}: {error:#}", path.display())),
            }
        }
        self.status = format!("{completed} moved to trash · {} failed", errors.len());
        if let Some(error) = errors.first() {
            self.status.push_str(&format!(" · {error}"));
        }
        self.refresh()
    }
}

pub fn trash_path(path: &Path) -> Result<()> {
    if trash_command(path).is_ok() {
        return Ok(());
    }
    trash_dir(path)
}

/// Prefer a system trash helper so deletions can be undone outside the app.
fn trash_command(path: &Path) -> Result<()> {
    let mut attempts: Vec<(String, Vec<String>)> = vec![("gio".into(), vec!["trash".into()])];
    if let Some(xdg) = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
    {
        attempts.push((
            "gio".into(),
            vec![
                "trash".into(),
                "--use-trash-dir".into(),
                format!("{xdg}/Trash"),
            ],
        ));
    }
    for (program, args) in attempts {
        let status = Command::new(&program)
            .args(&args)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|s| s.success()) {
            return Ok(());
        }
    }
    bail!("no system trash helper (gio/trash) available")
}

/// XDG trash fallback: move into ~/.local/share/Trash with a .trashinfo sidecar.
fn trash_dir(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .context("cannot trash the filesystem root")?;
    let data = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| option_sdk::home_dir().join(".local/share"));
    let files = data.join("Trash/files");
    let info = data.join("Trash/info");
    fs::create_dir_all(&files)
        .with_context(|| format!("cannot create trash at {}", files.display()))?;
    fs::create_dir_all(&info)
        .with_context(|| format!("cannot create trash at {}", info.display()))?;

    let target = unique_path(&files.join(name));
    let trash_name = target.file_name().context("invalid trash target")?;
    let info_path = info.join(format!(
        "{}.trashinfo",
        trash_name.to_string_lossy().replace(".trashinfo", "")
    ));
    let original = path
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string());
    let timestamp = chrono_ish_timestamp();
    fs::write(
        &info_path,
        format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            original.replace('\n', "%0A"),
            timestamp
        ),
    )
    .with_context(|| format!("cannot write {}", info_path.display()))?;

    match fs::rename(path, &target) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Cross-device fallback: copy into the trash, then remove the source.
            copy_recursively(path, &target)?;
            remove_path(path)?;
            Ok(())
        }
    }
}

/// RFC 3339 timestamp without extra dependencies, e.g. 2026-08-12T14:30:00.
fn chrono_ish_timestamp() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn validate_entry_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("name is empty");
    }
    if name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        bail!("name must be a single path segment");
    }
    let path = Path::new(name);
    if path.is_absolute() {
        bail!("name must be a single path segment");
    }
    let mut components = path.components();
    match components.next() {
        Some(std::path::Component::Normal(_)) => {}
        _ => bail!("name must be a single path segment"),
    }
    if components.next().is_some() {
        bail!("name must be a single path segment");
    }
    Ok(())
}

pub fn remove_path(path: &Path) -> Result<()> {
    if path.symlink_metadata()?.is_dir() {
        fs::remove_dir_all(path)?
    } else {
        fs::remove_file(path)?
    };
    Ok(())
}

pub fn unique_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("copy");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1.. {
        let name = match ext {
            Some(ext) => format!("{stem} copy {n}.{ext}"),
            None => format!("{stem} copy {n}"),
        };
        let candidate = parent.join(&name);
        if candidate.symlink_metadata().is_err()
            && parent
                .join(format!("{name}.trashinfo"))
                .symlink_metadata()
                .is_err()
        {
            return candidate;
        }
    }
    unreachable!()
}

/// Score how well `terms` appear as a subsequence of `name` (all lowercase).
/// Returns None when any term is missing. Consecutive matches and camelCase
/// boundaries score higher, so results read like fzf ranking.
pub fn fuzzy_score(name: &str, terms: &[&str]) -> Option<i64> {
    let mut score = 0i64;
    let mut pos = 0usize;
    let bytes = name.as_bytes();
    for term in terms.iter() {
        let needle = term.as_bytes();
        if needle.is_empty() {
            continue;
        }
        let start = pos;
        let mut matches: Vec<usize> = Vec::with_capacity(needle.len());
        for &b in needle {
            while pos < bytes.len() && bytes[pos] != b {
                pos += 1;
            }
            if pos >= bytes.len() {
                return None;
            }
            matches.push(pos);
            pos += 1;
        }
        // Consecutive run in the original name is the strongest signal.
        if matches.windows(2).all(|w| w[1] == w[0] + 1) {
            score += 100;
        }
        // Word/camelCase boundary bonuses.
        for &m in &matches {
            let prev = m.checked_sub(1).and_then(|i| bytes.get(i)).copied();
            let boundary = m == 0
                || prev.is_none_or(|p| {
                    p == b'_' || p == b'-' || p == b' ' || p == b'.' || p.is_ascii_uppercase()
                });
            if boundary {
                score += 10;
            }
        }
        // Prefer terms appearing earlier in the name.
        score -= (start as i64).min(256);
    }
    // Fewer, longer terms beat many scattered matches.
    score -= (terms.len() as i64) * 8;
    // Tie-breaker: shorter names win on equal match quality.
    score -= (name.len() as i64) / 2;
    // Keep names stable relative to the original ordering on equal scores.
    Some(score)
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
        let report = run_transfer(
            &TransferRequest {
                sources: app.selected_paths(),
                destination: root.clone(),
                mode: ClipboardMode::Copy,
            },
            &AtomicBool::new(false),
            |_| {},
            |_| ConflictResolution {
                action: ConflictAction::KeepBoth,
                apply_to_batch: true,
            },
        );
        assert!(report.errors.is_empty());
        assert!(root.join("notes.txt").exists());
        assert!(root.join("notes copy 1.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filters_entries_and_restores_full_directory() {
        let root = sandbox();
        fs::write(root.join("alpha.txt"), "a").unwrap();
        fs::write(root.join("beta.txt"), "b").unwrap();
        let mut app = App::new(&root, false).unwrap();
        app.set_filter("ALP".into()).unwrap();
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].name, "alpha.txt");
        app.set_filter(String::new()).unwrap();
        assert_eq!(app.entries.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_parent_dir_names() {
        let root = sandbox();
        let mut app = App::new(&root, false).unwrap();
        assert!(app.create_file("../escape.txt").is_err());
        assert!(app.create_dir("..").is_err());
        assert!(!root.join("escape.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_nested_relative_names() {
        let root = sandbox();
        let mut app = App::new(&root, false).unwrap();
        assert!(app.create_file("a/b.txt").is_err());
        assert!(app.create_dir("nested/dir").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_escape_on_rename() {
        let root = sandbox();
        let mut app = App::new(&root, false).unwrap();
        app.create_file("ok.txt").unwrap();
        app.selected = app.entries.iter().position(|e| e.name == "ok.txt").unwrap();
        assert!(app.rename_current("../x").is_err());
        assert!(root.join("ok.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fuzzy_ranks_better_matches_first() {
        let mut scored: Vec<(i64, &str)> = ["draft-notes.txt", "draft.txt", "notes-draft.txt"]
            .iter()
            .filter_map(|name| fuzzy_score(name, &["drft"]).map(|score| (score, *name)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        assert_eq!(scored[0].1, "draft.txt");

        // Multi-term query requires all terms present as subsequences.
        assert!(fuzzy_score("draft-notes.txt", &["src", "main"]).is_none());
        assert!(fuzzy_score("source-main.rs", &["src", "main"]).is_some());
    }

    #[test]
    fn fuzzy_prefers_earlier_and_boundary_matches() {
        let a = fuzzy_score("xxconfigxx", &["conf"]).unwrap();
        let b = fuzzy_score("configxx", &["conf"]).unwrap();
        assert!(b > a, "earlier match should rank higher");
    }
}
