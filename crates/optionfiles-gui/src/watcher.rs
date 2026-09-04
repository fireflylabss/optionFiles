//! Directory watcher: observes the current folder and asks the main loop
//! to refresh.
//!
//! The core has no observer — `App::refresh()` re-scans the disk. This watcher
//! uses `notify` to detect external changes and signals the refresh with a
//! simple debounce: a burst of writes becomes a single refresh.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glib::source::timeout_add_local;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Non-recursive watch of one directory: emits `on_change` on the main
/// thread after debouncing.
pub struct DirWatcher {
    _watcher: RecommendedWatcher,
}

impl DirWatcher {
    /// Watches `dir` (files directly inside it). `on_change` runs on the main
    /// thread (debounce timer callback), at most once per `debounce`.
    pub fn watch(
        dir: &Path,
        debounce: Duration,
        on_change: impl Fn() + 'static,
    ) -> Result<Self, notify::Error> {
        // The `notify` thread only flags activity; the timer does the debounce.
        let dirty = Arc::new(AtomicBool::new(false));
        let dirty_watch = dirty.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(ev) = res {
                    // Ignore access (read) events — only refresh on writes.
                    if !ev.kind.is_access() {
                        dirty_watch.store(true, Ordering::SeqCst);
                    }
                }
            })?;

        watcher.watch(dir, RecursiveMode::NonRecursive)?;

        // Debounce timer: runs on the main loop and only calls on_change when
        // something happened since the last check.
        timeout_add_local(debounce, move || {
            if dirty.swap(false, Ordering::SeqCst) {
                on_change();
            }
            glib::ControlFlow::Continue
        });

        Ok(Self { _watcher: watcher })
    }
}
