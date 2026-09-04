//! Places persistence: user favorites.
//!
//! Favorites live in `~/.option/files/favorites.toml` as one string array,
//! read and written with a tiny hand-rolled TOML layer for that exact shape
//! (no new dependencies). Writes go through `option_sdk::atomic_write`, and
//! every call here runs on a worker thread via `CoreBridge`.

use std::path::PathBuf;

fn files_dir() -> PathBuf {
    option_sdk::App::FILES.dir()
}

pub fn favorites_file() -> PathBuf {
    files_dir().join("favorites.toml")
}

/// Trash shown in the sidebar: the XDG trash contents, when they exist.
pub fn trash_files_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| option_sdk::home_dir().join(".local/share"))
        .join("Trash/files")
}

/// Localized user folders, resolved off the main thread: prefers the
/// Portuguese XDG names (Documentos, Imagens, …) when they exist, otherwise
/// the English ones. Never called on the main thread.
#[derive(Clone)]
pub struct XdgDirs {
    pub desktop: PathBuf,
    pub documents: PathBuf,
    pub downloads: PathBuf,
    pub pictures: PathBuf,
    pub videos: PathBuf,
    pub music: PathBuf,
    pub projects: PathBuf,
}

impl XdgDirs {
    /// English layout, no existence checks (sidebar placeholder until the
    /// worker resolves the real folders).
    pub fn fallback() -> Self {
        let home = option_sdk::home_dir();
        Self {
            desktop: home.join("Desktop"),
            documents: home.join("Documents"),
            downloads: home.join("Downloads"),
            pictures: home.join("Pictures"),
            videos: home.join("Videos"),
            music: home.join("Music"),
            projects: home.join("Projects"),
        }
    }
}

pub fn resolve_xdg() -> XdgDirs {
    let home = option_sdk::home_dir();
    let pick = |localized: &str, english: &str| {
        let primary = home.join(localized);
        if primary.is_dir() {
            primary
        } else {
            home.join(english)
        }
    };
    XdgDirs {
        desktop: home.join("Desktop"),
        documents: pick("Documentos", "Documents"),
        downloads: home.join("Downloads"),
        pictures: pick("Imagens", "Pictures"),
        videos: pick("Vídeos", "Videos"),
        music: pick("Músicas", "Music"),
        projects: home.join("Projects"),
    }
}

/// Reads a `key = [ "a", "b" ]` string array. Missing files and broken
/// content both mean "empty", never an error.
pub fn read_string_array(path: &std::path::Path, key: &str) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    parse_string_array(&text, key)
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn parse_string_array(text: &str, key: &str) -> Vec<String> {
    // Finds `key = [` then collects quoted strings until `]`.
    let mut out = Vec::new();
    let key_pos = match text.find(key) {
        Some(i) => i,
        None => return out,
    };
    let start = match text[key_pos..].find('[') {
        Some(rel) => rel,
        None => return out,
    };
    let mut rest = &text[key_pos + start + 1..];
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.starts_with(']') || rest.is_empty() {
            break;
        }
        if !rest.starts_with('"') {
            // Skip one line of unexpected content, keep parsing the rest.
            rest = rest.split_once('\n').map(|(_, tail)| tail).unwrap_or("");
            continue;
        }
        let mut value = String::new();
        let mut chars = rest[1..].chars();
        let mut closed = false;
        let mut consumed = 1usize;
        while let Some(c) = chars.next() {
            consumed += c.len_utf8();
            match c {
                '\\' => match chars.next() {
                    Some('n') => {
                        consumed += 1;
                        value.push('\n');
                    }
                    Some('t') => {
                        consumed += 1;
                        value.push('\t');
                    }
                    Some(e) => {
                        consumed += e.len_utf8();
                        value.push(e);
                    }
                    None => break,
                },
                '"' => {
                    closed = true;
                    break;
                }
                _ => value.push(c),
            }
        }
        rest = &rest[consumed..];
        if closed {
            out.push(value);
        } else {
            break;
        }
    }
    out
}

fn render_string_array(key: &str, values: &[PathBuf]) -> String {
    let mut text = format!("{key} = [\n");
    for value in values {
        let escaped = value
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t");
        text.push_str(&format!("  \"{escaped}\",\n"));
    }
    text.push_str("]\n");
    text
}

pub fn load_favorites() -> Vec<PathBuf> {
    read_string_array(&favorites_file(), "paths")
}

pub fn save_favorites(paths: &[PathBuf]) -> std::io::Result<()> {
    let _ = option_sdk::App::FILES.ensure();
    option_sdk::atomic_write(favorites_file(), render_string_array("paths", paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths_with_spaces() {
        let paths = vec![
            PathBuf::from("/home/user/My Documents"),
            PathBuf::from("/tmp/we\"ird\\name"),
        ];
        let text = render_string_array("paths", &paths);
        assert_eq!(
            parse_string_array(&text, "paths"),
            paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn missing_file_reads_empty() {
        assert!(
            read_string_array(PathBuf::from("/no/such/file.toml").as_path(), "paths").is_empty()
        );
    }
}
