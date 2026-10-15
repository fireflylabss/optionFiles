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
fn copy_into_descendant_and_overwrite_self_leave_source_untouched() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.path("source/child")).unwrap();
    fs::write(sandbox.path("source/keep"), "precious").unwrap();
    let cancel = AtomicBool::new(false);
    let nested = run_transfer(
        &request(
            vec![sandbox.path("source")],
            &sandbox.path("source/child"),
            ClipboardMode::Copy,
        ),
        &cancel,
        |_| {},
        |_| resolution(ConflictAction::Overwrite),
    );
    assert_eq!(nested.errors.len(), 1);
    assert!(!sandbox.path("source/child/source").exists());
    let same = run_transfer(
        &request(vec![sandbox.path("source")], &sandbox.0, ClipboardMode::Cut),
        &cancel,
        |_| {},
        |_| resolution(ConflictAction::Overwrite),
    );
    assert_eq!(same.errors.len(), 1);
    assert_eq!(
        fs::read_to_string(sandbox.path("source/keep")).unwrap(),
        "precious"
    );
}

#[test]
fn batch_conflict_choice_applies_once_and_preserves_both_originals() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("destination")).unwrap();
    for name in ["a.txt", "b.txt"] {
        fs::write(sandbox.path(name), "new").unwrap();
        fs::write(sandbox.path("destination").join(name), "old").unwrap();
    }
    let mut prompts = 0;
    let report = run_transfer(
        &request(
            vec![sandbox.path("a.txt"), sandbox.path("b.txt")],
            &sandbox.path("destination"),
            ClipboardMode::Copy,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| {
            prompts += 1;
            ConflictResolution {
                action: ConflictAction::KeepBoth,
                apply_to_batch: true,
            }
        },
    );
    assert_eq!(prompts, 1);
    assert_eq!(report.completed.len(), 2);
    for name in ["a.txt", "b.txt"] {
        assert_eq!(
            fs::read_to_string(sandbox.path("destination").join(name)).unwrap(),
            "old"
        );
    }
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/a copy 1.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/b copy 1.txt")).unwrap(),
        "new"
    );
}

#[test]
fn skip_does_not_remove_move_source_and_batch_continues_after_failure() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("destination")).unwrap();
    fs::write(sandbox.path("skip"), "source").unwrap();
    fs::write(sandbox.path("ok"), "success").unwrap();
    fs::write(sandbox.path("destination/skip"), "existing").unwrap();
    let report = run_transfer(
        &request(
            vec![
                sandbox.path("missing"),
                sandbox.path("skip"),
                sandbox.path("ok"),
            ],
            &sandbox.path("destination"),
            ClipboardMode::Cut,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| resolution(ConflictAction::Skip),
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.skipped, vec![sandbox.path("skip")]);
    assert_eq!(report.completed, vec![sandbox.path("ok")]);
    assert_eq!(fs::read_to_string(sandbox.path("skip")).unwrap(), "source");
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/skip")).unwrap(),
        "existing"
    );
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/ok")).unwrap(),
        "success"
    );
    assert!(!sandbox.path("ok").exists());
}

#[test]
fn cancellation_mid_copy_preserves_source_and_existing_destination() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("destination")).unwrap();
    let data = vec![42u8; 256 * 1024];
    fs::write(sandbox.path("large"), &data).unwrap();
    fs::write(sandbox.path("destination/large"), "existing").unwrap();
    let cancel = AtomicBool::new(false);
    let report = run_transfer(
        &request(
            vec![sandbox.path("large")],
            &sandbox.path("destination"),
            ClipboardMode::Copy,
        ),
        &cancel,
        |progress| {
            if progress.bytes_done > 0 {
                cancel.store(true, Ordering::Release);
            }
        },
        |_| resolution(ConflictAction::Overwrite),
    );
    assert!(report.cancelled);
    assert!(report.completed.is_empty());
    assert_eq!(fs::read(sandbox.path("large")).unwrap(), data);
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/large")).unwrap(),
        "existing"
    );
    assert_eq!(
        fs::read_dir(sandbox.path("destination")).unwrap().count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn recursive_copy_failure_keeps_entire_source_and_old_destination() {
    use std::os::unix::net::UnixListener;
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("source")).unwrap();
    fs::create_dir_all(sandbox.path("destination/source")).unwrap();
    fs::write(sandbox.path("source/good"), "source data").unwrap();
    fs::write(sandbox.path("destination/source/old"), "old data").unwrap();
    let _socket = UnixListener::bind(sandbox.path("source/socket")).unwrap();
    let report = run_transfer(
        &request(
            vec![sandbox.path("source")],
            &sandbox.path("destination"),
            ClipboardMode::Copy,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| resolution(ConflictAction::Overwrite),
    );
    assert_eq!(report.errors.len(), 1);
    assert!(report.completed.is_empty());
    assert_eq!(
        fs::read_to_string(sandbox.path("source/good")).unwrap(),
        "source data"
    );
    assert!(sandbox.path("source/socket").symlink_metadata().is_ok());
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/source/old")).unwrap(),
        "old data"
    );
    assert!(!sandbox.path("destination/source/good").exists());
}

#[cfg(unix)]
#[test]
fn recursive_and_direct_symlink_copies_preserve_links_without_traversal() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("source")).unwrap();
    fs::create_dir(sandbox.path("destination")).unwrap();
    fs::create_dir(sandbox.path("outside")).unwrap();
    fs::write(sandbox.path("outside/precious"), "untouched").unwrap();
    symlink("../outside", sandbox.path("source/directory-link")).unwrap();
    symlink("missing-target", sandbox.path("source/broken-link")).unwrap();
    symlink("outside/precious", sandbox.path("file-link")).unwrap();
    let report = run_transfer(
        &request(
            vec![sandbox.path("source"), sandbox.path("file-link")],
            &sandbox.path("destination"),
            ClipboardMode::Copy,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| resolution(ConflictAction::KeepBoth),
    );
    assert!(report.errors.is_empty());
    assert_eq!(
        fs::read_link(sandbox.path("destination/source/directory-link")).unwrap(),
        PathBuf::from("../outside")
    );
    assert_eq!(
        fs::read_link(sandbox.path("destination/source/broken-link")).unwrap(),
        PathBuf::from("missing-target")
    );
    assert_eq!(
        fs::read_link(sandbox.path("destination/file-link")).unwrap(),
        PathBuf::from("outside/precious")
    );
    remove_path(&sandbox.path("source/directory-link")).unwrap();
    assert_eq!(
        fs::read_to_string(sandbox.path("outside/precious")).unwrap(),
        "untouched"
    );
}

#[cfg(unix)]
#[test]
fn same_filesystem_move_preserves_hardlink_identity() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("destination")).unwrap();
    fs::write(sandbox.path("source"), "before").unwrap();
    fs::hard_link(sandbox.path("source"), sandbox.path("other-link")).unwrap();
    let report = run_transfer(
        &request(
            vec![sandbox.path("source")],
            &sandbox.path("destination"),
            ClipboardMode::Cut,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| resolution(ConflictAction::Overwrite),
    );
    assert!(report.errors.is_empty());
    assert!(!sandbox.path("source").exists());
    fs::write(sandbox.path("destination/source"), "after").unwrap();
    assert_eq!(
        fs::read_to_string(sandbox.path("other-link")).unwrap(),
        "after"
    );
}

#[test]
fn parent_and_child_move_selection_transfers_once_without_partial_failure() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("source")).unwrap();
    fs::create_dir(sandbox.path("destination")).unwrap();
    fs::write(sandbox.path("source/child"), "child data").unwrap();
    let report = run_transfer(
        &request(
            vec![sandbox.path("source/child"), sandbox.path("source")],
            &sandbox.path("destination"),
            ClipboardMode::Cut,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| resolution(ConflictAction::KeepBoth),
    );
    assert!(report.errors.is_empty());
    assert_eq!(report.completed, vec![sandbox.path("source")]);
    assert_eq!(report.skipped, vec![sandbox.path("source/child")]);
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/source/child")).unwrap(),
        "child data"
    );
    assert!(!sandbox.path("source").exists());
}

#[test]
fn overwrite_moves_complete_directory_without_obsolete_entries() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.path("source/nested")).unwrap();
    fs::create_dir_all(sandbox.path("destination/source")).unwrap();
    fs::write(sandbox.path("source/nested/data"), "complete data").unwrap();
    fs::write(sandbox.path("destination/source/obsolete"), "old data").unwrap();
    let mut items_done = 0;
    let report = run_transfer(
        &request(
            vec![sandbox.path("source")],
            &sandbox.path("destination"),
            ClipboardMode::Cut,
        ),
        &AtomicBool::new(false),
        |progress| {
            items_done = progress.items_done;
        },
        |_| resolution(ConflictAction::Overwrite),
    );
    assert!(report.errors.is_empty());
    assert_eq!(report.completed, vec![sandbox.path("source")]);
    assert_eq!(items_done, 1);
    assert!(!sandbox.path("source").exists());
    assert!(!sandbox.path("destination/source/obsolete").exists());
    assert_eq!(
        fs::read_to_string(sandbox.path("destination/source/nested/data")).unwrap(),
        "complete data"
    );
}

#[cfg(unix)]
#[test]
fn symlink_destination_cannot_bypass_descendant_protection() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.path("source/nested")).unwrap();
    fs::write(sandbox.path("source/keep"), "keep data").unwrap();
    symlink("source/nested", sandbox.path("alias")).unwrap();
    let report = run_transfer(
        &request(
            vec![sandbox.path("source")],
            &sandbox.path("alias"),
            ClipboardMode::Cut,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| resolution(ConflictAction::Overwrite),
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(
        fs::read_to_string(sandbox.path("source/keep")).unwrap(),
        "keep data"
    );
    assert!(!sandbox.path("source/nested/source").exists());
}

#[cfg(target_os = "linux")]
fn cross_device_destination(source: &Sandbox) -> Option<Sandbox> {
    use std::os::unix::fs::MetadataExt;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = PathBuf::from("/dev/shm").join(format!(
        "optionfiles-cross-device-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).ok()?;
    let destination = Sandbox(path);
    if fs::metadata(&source.0).ok()?.dev() == fs::metadata(&destination.0).ok()?.dev() {
        return None;
    }
    Some(destination)
}

#[cfg(target_os = "linux")]
#[test]
fn cross_device_cancel_preserves_move_source_and_old_destination() {
    let source = Sandbox::new();
    let Some(destination) = cross_device_destination(&source) else {
        return;
    };
    let contents = vec![42; 256 * 1024];
    fs::write(source.path("large"), &contents).unwrap();
    fs::write(destination.path("large"), "old").unwrap();
    let cancel = AtomicBool::new(false);
    let report = run_transfer(
        &request(
            vec![source.path("large")],
            &destination.0,
            ClipboardMode::Cut,
        ),
        &cancel,
        |progress| {
            if progress.bytes_done > 0 {
                cancel.store(true, Ordering::Release);
            }
        },
        |_| resolution(ConflictAction::Overwrite),
    );
    assert!(report.cancelled);
    assert!(report.completed.is_empty());
    assert_eq!(fs::read(source.path("large")).unwrap(), contents);
    assert_eq!(
        fs::read_to_string(destination.path("large")).unwrap(),
        "old"
    );
    assert_eq!(fs::read_dir(&destination.0).unwrap().count(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn cross_device_failure_does_not_remove_move_source() {
    use std::os::unix::net::UnixListener;
    let source = Sandbox::new();
    let Some(destination) = cross_device_destination(&source) else {
        return;
    };
    fs::create_dir(source.path("folder")).unwrap();
    fs::write(source.path("folder/keep"), "source data").unwrap();
    let _socket = UnixListener::bind(source.path("folder/socket")).unwrap();
    fs::create_dir(destination.path("folder")).unwrap();
    fs::write(destination.path("folder/old"), "existing data").unwrap();
    let report = run_transfer(
        &request(
            vec![source.path("folder")],
            &destination.0,
            ClipboardMode::Cut,
        ),
        &AtomicBool::new(false),
        |_| {},
        |_| resolution(ConflictAction::Overwrite),
    );
    assert_eq!(report.errors.len(), 1);
    assert!(report.completed.is_empty());
    assert_eq!(
        fs::read_to_string(source.path("folder/keep")).unwrap(),
        "source data"
    );
    assert!(source.path("folder/socket").symlink_metadata().is_ok());
    assert_eq!(
        fs::read_to_string(destination.path("folder/old")).unwrap(),
        "existing data"
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
