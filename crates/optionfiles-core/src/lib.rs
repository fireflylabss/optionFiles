//! optionfiles-core — filesystem, app state and metadata helpers.
//!
//! Shared logic for the terminal CLI (`optionfiles-cli`) and a future GUI.
//! Keeps the monochrome philosophy: no terminal, CLI or GUI dependencies here.

pub mod app;
pub mod fs;
pub mod metadata;

pub use app::{
    App, Clipboard, ClipboardMode, clipboard_export, clipboard_import, fuzzy_score, remove_path,
    trash_path, unique_path, validate_entry_name,
};
pub use fs::{Entry, SortMode, copy_recursively, human_size, read_dir};
pub use metadata::format_time;
