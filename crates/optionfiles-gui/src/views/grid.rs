//! Central icon grid: `GtkGridView` over a `GtkSingleSelection`.
//!
//! The model holds one `StringObject` per row (names only); row data comes
//! from a shared entry vec indexed by `ListItem::position`, so no custom
//! GObjects are needed. Icons are theme-provided (`views::icons`), 64px in
//! the grid, with a two-line centered label. Dotfiles render dimmed.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use optionfiles_core::Entry;

use crate::views::icons;

/// Responsive icon grid (goes into the content paned).
pub struct IconGrid {
    /// Widget to place in the layout.
    pub root: gtk::ScrolledWindow,
    view: gtk::GridView,
    selection: gtk::SingleSelection,
    entries: Rc<RefCell<Vec<Entry>>>,
}

impl IconGrid {
    pub fn new() -> Self {
        let entries: Rc<RefCell<Vec<Entry>>> = Rc::new(RefCell::new(Vec::new()));
        let store = gio::ListStore::new::<gtk::StringObject>();
        let selection = gtk::SingleSelection::new(Some(store));
        selection.set_autoselect(false);
        selection.set_can_unselect(false);

        let factory = gtk::SignalListItemFactory::new();
        {
            let entries = entries.clone();
            factory.connect_setup(|_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    eprintln!("optionfiles-gtk: grid cell is not a ListItem");
                    return;
                };
                let base = gtk::Image::new();
                base.set_pixel_size(64);
                let overlay = gtk::Overlay::new();
                overlay.set_child(Some(&base));
                overlay.set_halign(gtk::Align::Center);
                overlay.set_valign(gtk::Align::Start);
                let emblem = gtk::Image::from_icon_name(icons::symlink_emblem());
                emblem.set_pixel_size(16);
                emblem.set_halign(gtk::Align::End);
                emblem.set_valign(gtk::Align::End);
                emblem.set_visible(false);
                overlay.add_overlay(&emblem);
                let label = gtk::Label::new(None);
                label.set_wrap(true);
                label.set_lines(2);
                label.set_justify(gtk::Justification::Center);
                label.set_halign(gtk::Align::Center);
                label.set_max_width_chars(14);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                label.add_css_class("option-grid-label");
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                vbox.set_halign(gtk::Align::Center);
                vbox.set_valign(gtk::Align::Start);
                // Uniform cell width so the grid stays aligned as names vary.
                vbox.set_size_request(104, -1);
                vbox.append(&overlay);
                vbox.append(&label);
                item.set_child(Some(&vbox));
            });
            factory.connect_bind(move |_, obj| {
                let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(vbox) = item.child().and_downcast::<gtk::Box>() else {
                    eprintln!("optionfiles-gtk: grid cell holds no Box");
                    return;
                };
                let overlay = vbox.first_child().and_downcast::<gtk::Overlay>();
                let label = overlay
                    .as_ref()
                    .and_then(|overlay| overlay.next_sibling())
                    .and_downcast::<gtk::Label>();
                let entries = entries.borrow();
                if let (Some(overlay), Some(label), Some(entry)) =
                    (overlay, label, entries.get(item.position() as usize))
                {
                    if let Some(base) = overlay.child().and_downcast::<gtk::Image>() {
                        base.set_from_gicon(&icons::icon_for_entry(entry));
                        let base_ptr = base.as_ptr() as *const std::ffi::c_void;
                        // The emblem is the overlay child that is not the base.
                        let mut child = overlay.first_child();
                        while let Some(widget) = child {
                            child = widget.next_sibling();
                            if widget.is::<gtk::Image>()
                                && widget.as_ptr() as *const std::ffi::c_void != base_ptr
                            {
                                widget.set_visible(entry.is_symlink);
                            }
                        }
                    }
                    label.set_text(&entry.name);
                    vbox.set_tooltip_text(Some(&entry.name));
                    vbox.set_opacity(if icons::is_dotfile(entry) { 0.55 } else { 1.0 });
                }
            });
        }

        let view = gtk::GridView::new(Some(selection.clone()), Some(factory));
        view.set_single_click_activate(false);
        view.add_css_class("option-grid");

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
            .scroll_to(index as u32, gtk::ListScrollFlags::NONE, None);
    }

    /// Moves keyboard focus into the grid.
    pub fn focus(&self) {
        self.view.grab_focus();
    }

    /// Approximate grid columns at the current width (for j/k steps).
    /// Cell is 104px wide (+ ~8px grid padding), so divide by 112 to stay
    /// in sync with the responsive `GridView` auto-flow layout.
    pub fn columns(&self) -> usize {
        (self.root.width().max(112) / 112) as usize
    }
}
