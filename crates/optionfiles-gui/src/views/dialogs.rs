//! Dialogs: new folder / file, rename (text prompts), delete confirmation,
//! shortcuts and about.
//!
//! All prompts use `AdwAlertDialog` with an inline `Entry`, in plain wording.

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

/// Text prompt with one entry field. Confirm passes the trimmed text.
pub fn prompt_name(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    initial: &str,
    confirm_label: &str,
    on_confirm: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", confirm_label);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("confirm"));
    dialog.set_close_response("cancel");

    let entry_c = entry.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "confirm" {
            on_confirm(entry_c.text().trim().to_string());
        }
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}

/// Delete confirmation: names the entry, explains it goes to trash.
pub fn confirm_delete(parent: &impl IsA<gtk::Widget>, name: &str, on_confirm: impl Fn() + 'static) {
    let body = format!("Move {name} to trash? You can restore it from the trash.");
    let dialog = adw::AlertDialog::new(Some("Delete"), Some(&body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Move to trash");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "delete" {
            on_confirm();
        }
    });
    dialog.present(Some(parent));
}

/// Static shortcut list for the file manager actions (mirrors the TUI keys).
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Back / Forward", "Alt+Left / Alt+Right"),
    ("Up one folder", "Alt+Up"),
    ("Previous location", "TUI: -"),
    ("Home folder", "Ctrl+Home"),
    ("Refresh", "F5 or Ctrl+R"),
    ("Search", "Ctrl+F or /"),
    ("Move selection", "J / K or arrows"),
    ("First item", "g or Home"),
    ("Last item", "G or End"),
    ("Page up / down", "PgUp / PgDn"),
    ("Open", "Enter or L"),
    ("Show hidden files", "A or ."),
    ("Cycle sort", "S"),
    ("New folder", "Ctrl+N"),
    ("New file", "Ctrl+Shift+N"),
    ("Rename", "F2 or R"),
    ("Copy", "Ctrl+C"),
    ("Cut", "Ctrl+X"),
    ("Paste", "Ctrl+V"),
    ("Move to trash", "Delete or D"),
    ("Properties", "Alt+Enter"),
    ("Edit with $EDITOR", "E"),
    ("Close search", "Esc"),
    ("Close", "Q or Ctrl+Q"),
];

/// Keyboard shortcut reference.
pub fn show_shortcuts(parent: &adw::ApplicationWindow) {
    let dialog = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(400)
        .content_height(480)
        .build();

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list.set_margin_top(12);
    list.set_margin_bottom(12);
    list.set_margin_start(12);
    list.set_margin_end(12);

    for (label, accel) in SHORTCUTS {
        let row = adw::ActionRow::builder().title(*label).build();
        let key = gtk::Label::new(Some(accel));
        key.add_css_class("dim-label");
        key.add_css_class("numeric");
        row.add_suffix(&key);
        list.append(&row);
    }

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scroll));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));
}

/// File properties, mirroring the CLI `info` command: type, size, path,
/// permissions and owner plus the modified date.
pub fn show_properties(
    parent: &adw::ApplicationWindow,
    name: &str,
    kind: &str,
    size: &str,
    path: &str,
    mode: &str,
    owner: &str,
    modified: &str,
) {
    let dialog = adw::Dialog::builder()
        .title("Properties")
        .content_width(420)
        .content_height(380)
        .build();

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list.set_margin_top(12);
    list.set_margin_bottom(12);
    list.set_margin_start(12);
    list.set_margin_end(12);

    for (key, value) in [
        ("Name", name.to_string()),
        ("Type", kind.to_string()),
        ("Size", size.to_string()),
        ("Location", path.to_string()),
        ("Permissions", mode.to_string()),
        ("Owner", owner.to_string()),
        ("Modified", modified.to_string()),
    ] {
        let row = adw::ActionRow::builder().title(key).subtitle(value).build();
        list.append(&row);
    }

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scroll));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));
}

/// Standard about dialog.
pub fn show_about(parent: &adw::ApplicationWindow) {
    let about = adw::AboutDialog::builder()
        .application_name("optionFiles")
        .application_icon("system-file-manager")
        .developer_name("Firefly Labs")
        .version(env!("CARGO_PKG_VERSION"))
        .license_type(gtk::License::Apache20)
        .comments("Minimal file manager. Browse and manage local files.")
        .build();
    about.present(Some(parent));
}
