//! Places sidebar, Dolphin-style: narrow column of theme-icon rows.
//!
//! Fixed rows (Home, Favorites, Network, Trash, Desktop, Documents,
//! Downloads, Pictures, Videos, Music, Projects) plus mounted devices, then
//! the user's favorite folders. All icons are theme-provided at
//! 20px. Sections rebuild from plain data; navigation goes back to `app.rs`.

use std::path::PathBuf;

use gtk::gio;
use gtk::gio::prelude::*;
use gtk::prelude::*;

use crate::places;

/// Data the sidebar renders. Plain paths, no IO inside the widget.
pub struct SidebarData {
    pub cwd: PathBuf,
    pub favorites: Vec<PathBuf>,
    pub xdg: places::XdgDirs,
}

/// Narrow Places column (goes into the split-view sidebar slot).
pub struct PlacesBar {
    /// Widget to place in the layout.
    pub root: gtk::ScrolledWindow,
    body: gtk::Box,
}

impl PlacesBar {
    pub fn new() -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
        body.set_margin_top(8);
        body.set_margin_bottom(8);
        body.set_margin_start(6);
        body.set_margin_end(6);

        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        root.add_css_class("option-sidebar");
        root.set_child(Some(&body));
        Self { root, body }
    }

    /// Rebuilds every row from `data`.
    pub fn rebuild(
        &self,
        data: &SidebarData,
        on_navigate: impl Fn(PathBuf) + Clone + 'static,
        on_network: impl Fn() + Clone + 'static,
        on_remove_favorite: impl Fn(usize) + Clone + 'static,
    ) {
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        let home = option_sdk::home_dir();

        // Core places, in screenshot order: Pasta pessoal, Recentes,
        // Favoritos, Rede, Lixeira, then the XDG folders + Projects.
        // Recentes is visual-only for now (no core recents model), so it
        // never matches `cwd` and never steals the current-row highlight.
        let fixed: Vec<(String, String, SidebarTarget)> = vec![
            (
                "Pasta pessoal".into(),
                "user-home".into(),
                SidebarTarget::Dir(home.clone()),
            ),
            (
                "Recentes".into(),
                "document-open-recent".into(),
                SidebarTarget::Recents,
            ),
            (
                "Favoritos".into(),
                "starred".into(),
                SidebarTarget::Favorites,
            ),
            (
                "Rede".into(),
                "network-server".into(),
                SidebarTarget::Network,
            ),
            (
                "Lixeira".into(),
                "user-trash".into(),
                SidebarTarget::Dir(places::trash_files_dir()),
            ),
            (
                "Área de trabalho".into(),
                "user-desktop".into(),
                SidebarTarget::Dir(data.xdg.desktop.clone()),
            ),
            (
                "Documentos".into(),
                "folder-documents".into(),
                SidebarTarget::Dir(data.xdg.documents.clone()),
            ),
            (
                "Downloads".into(),
                "folder-download".into(),
                SidebarTarget::Dir(data.xdg.downloads.clone()),
            ),
            (
                "Imagens".into(),
                "folder-pictures".into(),
                SidebarTarget::Dir(data.xdg.pictures.clone()),
            ),
            (
                "Videos".into(),
                "folder-videos".into(),
                SidebarTarget::Dir(data.xdg.videos.clone()),
            ),
            (
                "Músicas".into(),
                "folder-music".into(),
                SidebarTarget::Dir(data.xdg.music.clone()),
            ),
            (
                "Projects".into(),
                "folder".into(),
                SidebarTarget::Dir(data.xdg.projects.clone()),
            ),
        ];
        self.append_rows(
            &fixed,
            &data.cwd,
            on_navigate.clone(),
            on_network.clone(),
            &data.favorites,
            None::<fn(usize)>,
        );

        // Devices: mounts known right now (external drives included).
        let mut devices: Vec<(String, String, SidebarTarget)> = Vec::new();
        for mount in gio::VolumeMonitor::get().mounts() {
            let name = mount.name().to_string();
            if name.is_empty() {
                continue;
            }
            if let Some(path) = mount.root().path() {
                devices.push((name, "drive-harddisk".to_string(), SidebarTarget::Dir(path)));
            }
        }
        if !devices.is_empty() {
            self.append_rows(
                &devices,
                &data.cwd,
                on_navigate.clone(),
                on_network.clone(),
                &data.favorites,
                None::<fn(usize)>,
            );
        }

        // User favorites, as real folders.
        if !data.favorites.is_empty() {
            self.append_header("Favoritos");
            let rows: Vec<(String, String, SidebarTarget)> = data
                .favorites
                .iter()
                .map(|p| {
                    (
                        short_name(p),
                        "folder".to_string(),
                        SidebarTarget::Dir(p.clone()),
                    )
                })
                .collect();
            self.append_rows(
                &rows,
                &data.cwd,
                on_navigate.clone(),
                on_network.clone(),
                &data.favorites,
                Some(on_remove_favorite.clone()),
            );
        }

        // Bottom dim truncated path, like the screenshot footer.
        let footer = gtk::Label::new(Some(&data.cwd.display().to_string()));
        footer.set_halign(gtk::Align::Start);
        footer.add_css_class("dim-label");
        footer.add_css_class("caption");
        footer.add_css_class("option-path-footer");
        footer.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        footer.set_single_line_mode(true);
        footer.set_margin_start(8);
        footer.set_margin_end(8);
        footer.set_margin_top(8);
        self.body.append(&footer);
    }

    fn append_header(&self, title: &str) {
        let label = gtk::Label::new(Some(title));
        label.set_halign(gtk::Align::Start);
        label.add_css_class("dim-label");
        label.add_css_class("caption");
        label.set_margin_start(8);
        label.set_margin_top(8);
        self.body.append(&label);
    }

    /// One block of rows. Favorite rows get right-click removal.
    #[allow(clippy::too_many_arguments)]
    fn append_rows<N, W, R>(
        &self,
        rows: &[(String, String, SidebarTarget)],
        cwd: &PathBuf,
        on_navigate: N,
        on_network: W,
        favorites: &[PathBuf],
        on_remove_favorite: Option<R>,
    ) where
        N: Fn(PathBuf) + Clone + 'static,
        W: Fn() + Clone + 'static,
        R: Fn(usize) + Clone + 'static,
    {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("option-places");
        for (label, icon_name, target) in rows {
            let row = gtk::ListBoxRow::new();
            row.set_activatable(true);
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_top(3);
            hbox.set_margin_bottom(3);
            hbox.set_margin_start(8);
            hbox.set_margin_end(8);
            let icon = gtk::Image::from_icon_name(icon_name);
            icon.set_pixel_size(20);
            icon.add_css_class("option-place-icon");
            let name_label = gtk::Label::new(Some(label));
            name_label.set_halign(gtk::Align::Start);
            name_label.set_hexpand(true);
            name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name_label.set_single_line_mode(true);
            name_label.add_css_class("option-place-label");
            hbox.append(&icon);
            hbox.append(&name_label);
            if matches!(target, SidebarTarget::Dir(p) if p == cwd) {
                row.add_css_class("option-place-current");
            }
            row.set_child(Some(&hbox));
            list.append(&row);
        }

        // Resolve taps: fixed rows jump to their folder (first favorite
        // for the group row), Network shows a notice.
        let taps: Vec<SidebarTap> = rows
            .iter()
            .map(|(_, _, target)| match target {
                SidebarTarget::Dir(path) => SidebarTap::Dir(path.clone()),
                SidebarTarget::Network => SidebarTap::Network,
                SidebarTarget::Recents => SidebarTap::None,
                SidebarTarget::Favorites => favorites
                    .first()
                    .map(|p| SidebarTap::Dir(p.clone()))
                    .unwrap_or(SidebarTap::None),
            })
            .collect();
        let taps = std::rc::Rc::new(taps);
        let on_navigate = on_navigate.clone();
        let on_network = on_network.clone();
        list.connect_row_activated(move |_, row| {
            if let Some(tap) = taps.get(row.index() as usize) {
                match tap {
                    SidebarTap::Dir(path) => on_navigate(path.clone()),
                    SidebarTap::Network => on_network(),
                    SidebarTap::None => {}
                }
            }
        });

        // Right-click removal on user favorite rows.
        if let Some(remove) = on_remove_favorite {
            let mut index = 0usize;
            let mut child = list.first_child();
            while let Some(row) = child {
                child = row.next_sibling();
                let row_index = index;
                index += 1;
                let Some(row) = row.downcast_ref::<gtk::ListBoxRow>() else {
                    continue;
                };
                let remove = remove.clone();
                let gesture = gtk::GestureClick::new();
                gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
                let weak_row: gtk::glib::WeakRef<gtk::ListBoxRow> = row.downgrade();
                gesture.connect_pressed(move |_, _, _, _| {
                    let Some(row) = weak_row.upgrade() else {
                        return;
                    };
                    let popover = gtk::Popover::new();
                    popover.set_parent(&row);
                    let button = gtk::Button::with_label("Remove from favorites");
                    button.add_css_class("flat");
                    let popover_c = popover.clone();
                    let remove_c = remove.clone();
                    button.connect_clicked(move |_| {
                        popover_c.popdown();
                        remove_c(row_index);
                    });
                    popover.set_child(Some(&button));
                    popover.popup();
                });
                row.add_controller(gesture);
            }
        }

        self.body.append(&list);
    }
}

#[derive(Clone)]
enum SidebarTarget {
    Dir(PathBuf),
    Network,
    Favorites,
    Recents,
}

#[derive(Clone)]
enum SidebarTap {
    Dir(PathBuf),
    Network,
    None,
}

fn short_name(path: &PathBuf) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
