//! Bridge between the GTK main loop and `optionfiles-core` (synchronous).
//!
//! The core is fully pull-based and blocking. To keep the UI responsive every
//! call runs on a worker thread (`std::thread::spawn`) and the result is
//! delivered back on the main thread via `glib::MainContext::invoke`, which
//! schedules the callback on GTK's main context.

use glib::MainContext;

/// Runs core operations on worker threads and delivers results on the main loop.
pub struct CoreBridge;

impl CoreBridge {
    /// Runs `f` on a worker thread and calls `on_result` on the main thread
    /// when it finishes.
    pub fn call<F, T>(f: F, on_result: impl FnOnce(T) + Send + 'static)
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let context = MainContext::default();
        std::thread::spawn(move || {
            let out = f();
            context.invoke(move || on_result(out));
        });
    }
}
