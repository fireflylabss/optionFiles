//! Breadcrumb pill: one wide rounded pill with the current folder (`🏠`
//! home icon + display name, `Pasta pessoal` for home), a `…` button on the
//! right edge for view options, and the ancestor chain in a popover.
//!
//! Display names are grounded and short; the popover keeps full navigation
//! (every ancestor is one click away).

use std::path::{Path, PathBuf};

use gtk::prelude::*;

/// Display name for a folder: `Pasta pessoal` for home, else the file name.
pub fn display_name(path: &Path) -> String {
    if path == option_sdk::home_dir() {
        return "Pasta pessoal".to_string();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Wide centered pill for the current folder.
pub struct CrumbPill {
    /// Widget to place in the header (hexpand).
    pub root: gtk::Box,
    open_label: gtk::Label,
    more_btn: gtk::MenuButton,
    ancestors: gtk::Popover,
}

impl CrumbPill {
    pub fn new() -> Self {
        // Left part: home icon + name, opens the ancestor chain.
        let home_icon = gtk::Image::from_icon_name("user-home");
        home_icon.set_pixel_size(16);
        let open_label = gtk::Label::new(Some(""));
        open_label.set_halign(gtk::Align::Center);
        open_label.set_hexpand(true);
        open_label.set_justify(gtk::Justification::Center);
        open_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        open_label.set_single_line_mode(true);
        let open_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        open_box.append(&home_icon);
        open_box.append(&open_label);
        let open_btn = gtk::Button::new();
        open_btn.add_css_class("flat");
        open_btn.set_child(Some(&open_box));
        open_btn.set_hexpand(true);

        // Right edge: `…` opens the view/sort options.
        let more_btn = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Sort and view options")
            .build();
        more_btn.add_css_class("flat");

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("option-pill");
        root.set_hexpand(true);
        root.append(&open_btn);
        root.append(&more_btn);

        let ancestors = gtk::Popover::new();
        ancestors.set_parent(&open_btn);
        let ancestors_c = ancestors.clone();
        open_btn.connect_clicked(move |_| {
            ancestors_c.popup();
        });

        Self {
            root,
            open_label,
            more_btn,
            ancestors,
        }
    }

    /// Attaches the view/sort options popover to the `…` button.
    pub fn set_options_popover(&self, popover: &gtk::Popover) {
        self.more_btn.set_popover(Some(popover));
    }

    /// Updates the pill label and rebuilds the ancestor popover for `path`.
    /// Opening the popover never steals keyboard focus from the grid: rows
    /// activate on click, and closing returns focus to the pill.
    pub fn set_path(&self, path: &Path, on_navigate: impl Fn(PathBuf) + Clone + 'static) {
        self.open_label.set_text(&display_name(path));

        // Ancestors from the folder itself up to the root.
        let mut chain: Vec<PathBuf> = Vec::new();
        let mut current = Some(path.to_path_buf());
        while let Some(dir) = current {
            chain.push(dir.clone());
            current = dir.parent().map(Path::to_path_buf);
            if chain.len() > 64 {
                break;
            }
        }

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        for (index, ancestor) in chain.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            row.set_activatable(true);
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);
            hbox.set_margin_start(10);
            hbox.set_margin_end(10);
            let icon = gtk::Image::from_icon_name(if index == chain.len() - 1 {
                "drive-harddisk"
            } else {
                "folder"
            });
            icon.set_pixel_size(16);
            let label = gtk::Label::new(Some(&display_name(ancestor)));
            label.set_halign(gtk::Align::Start);
            label.set_hexpand(true);
            hbox.append(&icon);
            hbox.append(&label);
            if index == 0 {
                row.set_sensitive(false);
            }
            row.set_child(Some(&hbox));
            list.append(&row);
        }
        let chain = std::rc::Rc::new(chain);
        let ancestors = self.ancestors.clone();
        list.connect_row_activated(move |_, row| {
            if let Some(target) = chain.get(row.index() as usize) {
                ancestors.popdown();
                on_navigate(target.clone());
            }
        });
        self.ancestors.set_child(Some(&list));
    }
}
