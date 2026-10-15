//! Filesystem, navigation, clipboard and transfer logic shared by the TUI and GTK app.

pub mod app;
pub mod clipboard;
pub mod fs;
pub mod metadata;
pub mod transfer;

pub use app::{App, fuzzy_score, remove_path, trash_path, unique_path, validate_entry_name};
pub use clipboard::{
    Clipboard, ClipboardMode, GNOME_COPIED_FILES, KDE_CUT_SELECTION, URI_LIST, clipboard_decode,
    clipboard_encode, clipboard_export, clipboard_import, parse_file_uri, path_uri,
};
pub use fs::{Entry, SortMode, copy_recursively, human_size, read_dir};
pub use metadata::format_time;
pub use transfer::{
    ConflictAction, ConflictResolution, TransferConflict, TransferError, TransferProgress,
    TransferReport, TransferRequest, run_transfer,
};
