//! Details view: `GtkColumnView` with Name / Size / Modified columns.
//!
//! Same model pattern as the icon grid (names in the store, rows by
//! position). Icons are theme-provided at 16px; dotfiles render dimmed.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use optionfiles_core::{Entry, format_time, human_size};

use crate::views::icons;

/// Name / Size / Modified columns (goes into the content paned).
pub struct DetailsView {
    /// Widget to place in the layout.
    pub root: gtk::ScrolledWindow,
    view: gtk::ColumnView,
    selection: gtk::SingleSelection,
    entries: Rc<RefCell<Vec<Entry>>>,
}

impl DetailsView {
    pub fn new() -> Self {
        let entries: Rc<RefCell<Vec<Entry>>> = Rc::new(RefCell::new(Vec::new()));
        let store = gio::ListStore::new::<gtk::StringObject>();
        let selection = gtk::SingleSelection::new(Some(store));
        selection.set_autoselect(false);
        selection.set_can_unselect(false);
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_single_click_activate(false);
        view.set_show_column_separators(true);

        let name_factory = gtk::SignalListItemFactory::new();
        {
            let entries = entries.clone();
            name_factory.connect_setup(|_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    eprintln!("optionfiles-gtk: details cell is not a ListItem");
                    return;
                };
                let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let icon = gtk::Image::new();
                icon.set_pixel_size(16);
                let label = gtk::Label::new(None);
                label.set_halign(gtk::Align::Start);
                label.set_hexpand(true);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_single_line_mode(true);
                hbox.append(&icon);
                hbox.append(&label);
                item.set_child(Some(&hbox));
            });
            name_factory.connect_bind(move |_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    eprintln!("optionfiles-gtk: details cell is not a ListItem");
                    return;
                };
                let Some(hbox) = item.child().and_downcast::<gtk::Box>() else {
                    eprintln!("optionfiles-gtk: name cell holds no Box");
                    return;
                };
                let icon = hbox.first_child().and_downcast::<gtk::Image>();
                let label = icon
                    .as_ref()
                    .and_then(|icon| icon.next_sibling())
                    .and_downcast::<gtk::Label>();
                let entries = entries.borrow();
                if let (Some(icon), Some(label), Some(entry)) =
                    (icon, label, entries.get(item.position() as usize))
                {
                    icon.set_from_gicon(&icons::icon_for_entry(entry));
                    label.set_text(&entry.name);
                    icon.set_opacity(if icons::is_dotfile(entry) { 0.55 } else { 1.0 });
                    label.set_opacity(if icons::is_dotfile(entry) { 0.55 } else { 1.0 });
                }
            });
        }
        let name_column = gtk::ColumnViewColumn::new(Some("Name"), Some(name_factory));
        name_column.set_expand(true);
        view.append_column(&name_column);

        let size_factory = gtk::SignalListItemFactory::new();
        {
            let entries = entries.clone();
            size_factory.connect_setup(|_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let label = gtk::Label::new(None);
                label.set_halign(gtk::Align::End);
                label.add_css_class("dim-label");
                label.add_css_class("numeric");
                item.set_child(Some(&label));
            });
            size_factory.connect_bind(move |_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(label) = item.child().and_downcast::<gtk::Label>() else {
                    eprintln!("optionfiles-gtk: size cell holds no Label");
                    return;
                };
                let entries = entries.borrow();
                label.set_text(
                    entries
                        .get(item.position() as usize)
                        .map(|e| {
                            if e.is_dir {
                                "—".to_string()
                            } else {
                                human_size(e.size)
                            }
                        })
                        .as_deref()
                        .unwrap_or(""),
                );
            });
        }
        view.append_column(&gtk::ColumnViewColumn::new(
            Some("Size"),
            Some(size_factory),
        ));

        let date_factory = gtk::SignalListItemFactory::new();
        {
            let entries = entries.clone();
            date_factory.connect_setup(|_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let label = gtk::Label::new(None);
                label.set_halign(gtk::Align::End);
                label.add_css_class("dim-label");
                label.add_css_class("numeric");
                item.set_child(Some(&label));
            });
            date_factory.connect_bind(move |_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(label) = item.child().and_downcast::<gtk::Label>() else {
                    eprintln!("optionfiles-gtk: date cell holds no Label");
                    return;
                };
                let entries = entries.borrow();
                label.set_text(
                    entries
                        .get(item.position() as usize)
                        .and_then(|e| e.modified)
                        .map(format_time)
                        .as_deref()
                        .unwrap_or(""),
                );
            });
        }
        view.append_column(&gtk::ColumnViewColumn::new(
            Some("Modified"),
            Some(date_factory),
        ));

        let root = gtk::ScrolledWindow::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&view));

        Self {
            root,
            view,
            selection,
            entries,
        }
    }

    /// Rebuilds the model. Selection signals fire synchronously inside this
    /// call, so callers guard re-entrancy (see `syncing` in `app.rs`).
    pub fn set_entries(&self, entries: &[Entry], selected: usize) {
        *self.entries.borrow_mut() = entries.to_vec();
        if let Some(model) = self.selection.model() {
            if let Some(store) = model.downcast_ref::<gio::ListStore>() {
                store.remove_all();
                for entry in entries {
                    store.append(&gtk::StringObject::new(&entry.name));
                }
            }
        }
        if entries.is_empty() {
            self.selection.set_selected(gtk::INVALID_LIST_POSITION);
        } else {
            self.selection
                .set_selected(selected.min(entries.len() - 1) as u32);
        }
    }

    /// Fires when the selection moves. Index only.
    pub fn on_select(&self, cb: impl Fn(usize) + 'static) {
        self.selection.connect_selected_notify(move |selection| {
            let position = selection.selected();
            if position != gtk::INVALID_LIST_POSITION {
                cb(position as usize);
            }
        });
    }

    /// Fires on double click / Enter.
    pub fn on_activate(&self, cb: impl Fn(usize) + 'static) {
        self.view
            .connect_activate(move |_, position| cb(position as usize));
    }

    /// Moves the visual selection and scrolls it into view.
    pub fn select_index(&self, index: usize) {
        self.selection.set_selected(index as u32);
        self.view
            .scroll_to(index as u32, None, gtk::ListScrollFlags::NONE, None);
    }

    /// Moves keyboard focus into the columns.
    pub fn focus(&self) {
        self.view.grab_focus();
    }
}
