use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use optionfiles_core::{
    App, Clipboard, ClipboardMode, ConflictAction, ConflictResolution, GNOME_COPIED_FILES,
    TransferRequest, URI_LIST, clipboard_decode, clipboard_encode, parse_file_uri, path_uri,
    remove_path, run_transfer,
};

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "optionfiles-regression-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create sandbox: {error}"),
            }
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn resolution(action: ConflictAction) -> ConflictResolution {
    ConflictResolution {
        action,
        apply_to_batch: false,
    }
}

fn request(sources: Vec<PathBuf>, destination: &Path, mode: ClipboardMode) -> TransferRequest {
    TransferRequest {
        sources,
        destination: destination.to_path_buf(),
        mode,
    }
}

#[test]
fn filtering_and_hidden_toggles_use_snapshot_until_explicit_refresh() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.path("Alpha.txt"), "a").unwrap();
    fs::write(sandbox.path("beta.txt"), "b").unwrap();
    fs::write(sandbox.path(".hidden"), "h").unwrap();
    let mut app = App::new(&sandbox.0, false).unwrap();
    fs::remove_file(sandbox.path("Alpha.txt")).unwrap();
    fs::write(sandbox.path("new.txt"), "n").unwrap();
    app.set_filter("ALP".into()).unwrap();
    assert_eq!(
        app.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha.txt"]
    );
    app.set_filter(String::new()).unwrap();
    assert_eq!(app.entries.len(), 2);
    app.toggle_hidden().unwrap();
    assert_eq!(app.entries.len(), 3);
    assert!(!app.entries.iter().any(|entry| entry.name == "new.txt"));
    app.refresh().unwrap();
    assert!(!app.entries.iter().any(|entry| entry.name == "Alpha.txt"));
    assert!(app.entries.iter().any(|entry| entry.name == "new.txt"));
}

#[test]
fn marked_selection_survives_filter_and_restores_with_directory_scroll() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("child")).unwrap();
    for name in ["alpha", "beta", "gamma"] {
        fs::write(sandbox.path(name), name).unwrap();
    }
    let mut app = App::new(&sandbox.0, false).unwrap();
    app.selected = app
        .entries
        .iter()
        .position(|entry| entry.name == "beta")
        .unwrap();
    app.toggle_selection();
    app.selected = app
        .entries
        .iter()
        .position(|entry| entry.name == "gamma")
        .unwrap();
    app.toggle_selection();
    app.scroll = 2;
    let focused = app.current().unwrap().path.clone();
    let marked = app.selected_paths();
    app.navigate(&sandbox.path("child")).unwrap();
    assert!(app.marked.is_empty());
    app.parent().unwrap();
    assert_eq!(app.current().unwrap().path, focused);
    assert_eq!(app.scroll, 2);
    assert_eq!(app.selected_paths(), marked);
    app.set_filter("alpha".into()).unwrap();
    assert_eq!(app.selected_paths(), marked);
    app.clear_selection();
    app.select_all();
    assert_eq!(app.selected_paths(), vec![sandbox.path("alpha")]);
}

#[test]
fn failed_navigation_keeps_active_snapshot_selection_and_location() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.path("keep"), "contents").unwrap();
    let mut app = App::new(&sandbox.0, false).unwrap();
    app.toggle_selection();
    let marked = app.selected_paths();
    assert!(app.navigate(&sandbox.path("missing")).is_err());
    assert_eq!(app.cwd, sandbox.0);
    assert_eq!(app.selected_paths(), marked);
    assert_eq!(app.current().unwrap().name, "keep");
}

#[test]
fn local_file_clipboards_roundtrip_all_paths_and_copy_cut_modes() {
    let paths = vec![
        PathBuf::from("/tmp/space name.txt"),
        PathBuf::from("/tmp/ação\nline#%.txt"),
    ];
    for mode in [ClipboardMode::Copy, ClipboardMode::Cut] {
        let clipboard = Clipboard {
            paths: paths.clone(),
            mode,
        };
        let encoded = clipboard_encode(&clipboard, GNOME_COPIED_FILES);
        assert_eq!(
            clipboard_decode(&encoded, GNOME_COPIED_FILES).unwrap(),
            clipboard
        );
        let uri = clipboard_encode(&clipboard, URI_LIST);
        assert_eq!(clipboard_decode(&uri, URI_LIST).unwrap().paths, paths);
        assert_eq!(
            clipboard_decode(&uri, URI_LIST).unwrap().mode,
            ClipboardMode::Copy
        );
    }
    assert_eq!(
        parse_file_uri("file://localhost/tmp/a%20b").unwrap(),
        PathBuf::from("/tmp/a b")
    );
    assert!(parse_file_uri("file://remote.example/tmp/a").is_err());
    assert!(parse_file_uri("https://example.com/a").is_err());
    assert!(parse_file_uri("file:///tmp/bad%00").is_err());
    assert!(parse_file_uri("file:///tmp/bad%2").is_err());
}

#[cfg(unix)]
#[test]
fn file_uri_preserves_non_utf8_and_symlink_identity() {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    let path = sandbox
        .0
        .join(std::ffi::OsString::from_vec(b"non-utf8-\xff".to_vec()));
    assert_eq!(parse_file_uri(&path_uri(&path)).unwrap(), path);
    fs::write(sandbox.path("target"), "target contents").unwrap();
    symlink("target", sandbox.path("link")).unwrap();
    assert_eq!(
        parse_file_uri(&path_uri(&sandbox.path("link"))).unwrap(),
        sandbox.path("link")
    );
}

#[test]
fn missing_initial_path_does_not_silently_open_its_parent() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.path("existing.txt"), "keep").unwrap();
    assert!(App::new(&sandbox.path("missing"), false).is_err());
    let app = App::new(&sandbox.path("existing.txt"), false).unwrap();
    assert_eq!(app.cwd, sandbox.0);
    assert_eq!(app.current().unwrap().name, "existing.txt");
}
