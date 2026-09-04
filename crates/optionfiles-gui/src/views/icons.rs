//! System icon lookup. Every icon comes from the current icon theme
//! (`ThemedIcon` / content-type icons via gio) — folder orange, file glyphs
//! and emblems are all theme-provided. Nothing is painted by CSS here.
//!
//! All lookups are name-based and allocation-free of IO: safe on the main
//! thread.

use std::path::Path;

use gtk::gio;
use gtk::gio::prelude::*;
use optionfiles_core::Entry;

/// Full-color themed icon for an entry.
pub fn icon_for_entry(entry: &Entry) -> gio::Icon {
    if entry.is_dir {
        return gio::ThemedIcon::new(special_folder_icon(&entry.path)).upcast();
    }
    // Name-based content-type guess (no file IO involved).
    let (content_type, _) = gio::content_type_guess(Some(&entry.name), &[]);
    gio::content_type_get_icon(content_type.as_str())
}

/// Theme icon for well-known folders (`folder` everywhere else, so the
/// system theme paints every directory its own orange).
pub fn special_folder_icon(path: &Path) -> &'static str {
    let home = option_sdk::home_dir();
    if path == home {
        return "user-home";
    }
    if home.join("Desktop") == path {
        return "user-desktop";
    }
    if home.join("Documents") == path {
        return "folder-documents";
    }
    if home.join("Downloads") == path {
        return "folder-download";
    }
    if home.join("Pictures") == path {
        return "folder-pictures";
    }
    if home.join("Videos") == path {
        return "folder-videos";
    }
    if home.join("Music") == path {
        return "folder-music";
    }
    "folder"
}

/// Whether an entry is a dotfile (rendered dimmed, like Dolphin).
pub fn is_dotfile(entry: &Entry) -> bool {
    entry.name.starts_with('.')
}

/// Emblem shown over symlinked items.
pub fn symlink_emblem() -> &'static str {
    "emblem-symbolic-link"
}
