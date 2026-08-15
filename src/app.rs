use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

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
    pub previous_dir: Option<PathBuf>,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub scroll: usize,
    pub show_hidden: bool,
    pub sort: SortMode,
    pub clipboard: Option<Clipboard>,
    pub status: String,
    pub help: bool,
    pub preview: bool,
    pub filter: String,
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
            show_hidden,
            sort: SortMode::Name,
            clipboard: None,
            status: String::new(),
            help: false,
            preview: true,
            filter: String::new(),
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
        if !self.filter.is_empty() {
            let needle = self.filter.to_lowercase();
            // Plain substring when the query is a single contiguous token,
            // fuzzy subsequence ranking otherwise (spaces or scattered letters).
            if needle.split_whitespace().count() == 1 && !needle.is_empty() {
                self.entries
                    .retain(|entry| entry.name.to_lowercase().contains(&needle));
            } else {
                let terms: Vec<&str> = needle.split_whitespace().collect();
                let mut scored: Vec<(i64, &Entry)> = self
                    .entries
                    .iter()
                    .filter_map(|entry| {
                        fuzzy_score(&entry.name.to_lowercase(), &terms).map(|score| (score, entry))
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0));
                self.entries = scored.into_iter().map(|(_, entry)| entry.clone()).collect();
            }
        }
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
            self.previous_dir = Some(self.cwd.clone());
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
        self.previous_dir = Some(old.clone());
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
    pub fn set_filter(&mut self, filter: String) -> Result<()> {
        self.filter = filter;
        self.selected = 0;
        self.scroll = 0;
        self.refresh()
    }
    pub fn go_home(&mut self) -> Result<()> {
        let home = option_sdk::home_dir();
        self.previous_dir = Some(self.cwd.clone());
        self.cwd = home
            .canonicalize()
            .with_context(|| format!("cannot open home {}", home.display()))?;
        self.selected = 0;
        self.scroll = 0;
        self.refresh()
    }
    pub fn go_previous(&mut self) -> Result<()> {
        let Some(previous) = self.previous_dir.take() else {
            self.status = "no previous location".into();
            return Ok(());
        };
        let current = std::mem::replace(&mut self.cwd, previous);
        self.previous_dir = Some(current);
        self.selected = 0;
        self.scroll = 0;
        self.refresh()
    }
    pub fn set_clipboard(&mut self, mode: ClipboardMode) {
        if let Some(entry) = self.current() {
            let path = entry.path.clone();
            self.clipboard = Some(Clipboard {
                path: path.clone(),
                mode,
            });
            let label = match mode {
                ClipboardMode::Copy => "copied",
                ClipboardMode::Cut => "cut",
            };
            match clipboard_export(&path) {
                Ok(Some(app)) => self.status = format!("{label} ({app})"),
                _ => self.status = label.into(),
            }
        }
    }
    pub fn paste(&mut self) -> Result<()> {
        let clip = match self.clipboard.clone() {
            Some(clip) => Some(clip),
            None => clipboard_import()
                .map(|path| Clipboard {
                    path,
                    mode: ClipboardMode::Copy,
                })
                .map(Some)
                .unwrap_or(None),
        };
        let Some(clip) = clip else {
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
    pub fn delete_current(&mut self) -> Result<()> {
        let entry = self.current().context("nothing selected")?.clone();
        trash_path(&entry.path)?;
        self.status = format!("moved {} to trash", entry.name);
        self.refresh()
    }
}

fn trash_path(path: &Path) -> Result<()> {
    if trash_command(path).is_ok() {
        return Ok(());
    }
    trash_dir(path)
}

/// Push the selected path into the system clipboard so it can be pasted in
/// other applications. Returns the helper name on success.
fn clipboard_export(path: &Path) -> Result<Option<&'static str>> {
    if cfg!(target_os = "macos") {
        return match Command::new("pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(path.to_string_lossy().as_bytes())?;
                child.wait()
            }) {
            Ok(status) if status.success() => Ok(Some("pbcopy")),
            _ => Ok(None),
        };
    }
    let attempts: Vec<(&str, Vec<&str>)> = vec![
        ("wl-copy", vec!["--type", "text/uri-list"]),
        ("xclip", vec!["-selection", "clipboard"]),
    ];
    for (program, args) in attempts {
        let mut child = match Command::new(program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => continue,
        };
        use std::io::Write;
        let uri = path_uri(path);
        let written = child
            .stdin
            .take()
            .map(|mut stdin| stdin.write_all(uri.as_bytes()))
            .is_some();
        let success = child.wait().is_ok_and(|s| s.success());
        if written && success {
            return Ok(Some(program));
        }
    }
    Ok(None)
}

/// Read a path from the system clipboard, if a helper is available and the
/// contents look like a file URI.
fn clipboard_import() -> Option<PathBuf> {
    for program in ["wl-paste", "xclip"] {
        let args: &[&str] = if program == "wl-paste" {
            &["--no-newline"]
        } else {
            &["-selection", "clipboard", "-o"]
        };
        let Ok(output) = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let text = text.trim().trim_matches('\0');
        if text.is_empty() {
            return None;
        }
        // file:// URIs, optionally with a trailing newline.
        let decoded = if let Some(rest) = text.strip_prefix("file://") {
            let rest = rest.trim_end_matches(['\r', '\n']);
            percent_decode(rest)
        } else {
            text.to_string()
        };
        let path = PathBuf::from(decoded);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn path_uri(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!(
        "file://{}",
        percent_encode(absolute.to_string_lossy().as_bytes())
    )
}

fn percent_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
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

fn validate_entry_name(name: &str) -> Result<()> {
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
        let candidate = parent.join(&name);
        if !candidate.exists() && !parent.join(format!("{name}.trashinfo")).exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// Score how well `terms` appear as a subsequence of `name` (all lowercase).
/// Returns None when any term is missing. Consecutive matches and camelCase
/// boundaries score higher, so results read like fzf ranking.
fn fuzzy_score(name: &str, terms: &[&str]) -> Option<i64> {
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
        app.set_clipboard(ClipboardMode::Copy);
        app.paste().unwrap();
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
