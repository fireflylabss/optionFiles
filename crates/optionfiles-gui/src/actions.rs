//! Window actions (`win.*`): navigation, views, clipboard, open/edit,
//! properties, CRUD dialogs, favorites, theme, help. Stateless
//! `SimpleAction`s delegating to `app.rs`.

use gtk::gio;
use gtk::prelude::*;
use libadwaita as adw;

use crate::app;

/// Registers every `win.*` action and its accelerator.
pub fn register(window: &adw::ApplicationWindow, app: &adw::Application) {
    let plain = |name: &str, run: fn()| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| run());
        window.add_action(&action);
    };

    // Navigation (Dolphin back/forward plus TUI up/previous/home).
    plain("nav-back", app::go_history_back);
    plain("nav-forward", app::go_history_forward);
    plain("up", app::go_up);
    plain("back", app::go_previous);
    plain("home", app::go_home);
    plain("refresh", app::refresh_manual);

    // Views.
    plain("view-icons", || app::set_view_icons(true));
    plain("view-details", || app::set_view_icons(false));
    plain("view-toggle", app::toggle_view);
    plain("toggle-sidebar", app::toggle_sidebar);
    plain("toggle-hidden", app::toggle_hidden);
    plain("cycle-sort", app::cycle_sort);

    // Selection movement (also reachable as plain keys in the browser).
    plain("select-first", app::select_first);
    plain("select-last", app::select_last);

    // Clipboard, open, edit, properties.
    plain("copy", app::copy_selected);
    plain("cut", app::cut_selected);
    plain("paste", app::paste_here);
    plain("open", app::open_selected);
    plain("edit", app::edit_selected);
    plain("properties", app::show_properties);

    // CRUD + favorites.
    plain("new-folder", app::show_new_folder);
    plain("new-file", app::show_new_file);
    plain("rename", app::show_rename);
    plain("delete", app::show_delete);
    plain("favorite-toggle", app::toggle_favorite);

    // Window.
    plain("focus-search", app::focus_search);
    plain("shortcuts", app::show_shortcuts);
    plain("about", app::show_about);
    plain("close", app::close_window);
    plain("theme-dark", || {
        app::set_theme(adw::ColorScheme::ForceDark);
    });
    plain("theme-system", || {
        app::set_theme(adw::ColorScheme::Default);
    });
    plain("theme-light", || {
        app::set_theme(adw::ColorScheme::ForceLight);
    });

    // Accelerators. Plain-letter TUI keys (j/k/g/…) intentionally stay out:
    // they are handled by the window key controller so typing in the filter
    // box never triggers actions.
    app.set_accels_for_action("win.nav-back", &["<Alt>Left"]);
    app.set_accels_for_action("win.nav-forward", &["<Alt>Right"]);
    app.set_accels_for_action("win.up", &["<Alt>Up", "BackSpace"]);
    app.set_accels_for_action("win.back", &["<Alt>Home"]);
    app.set_accels_for_action("win.home", &["<Primary>Home"]);
    app.set_accels_for_action("win.refresh", &["F5", "<Primary>r"]);
    app.set_accels_for_action("win.focus-search", &["<Primary>f", "slash"]);
    app.set_accels_for_action("win.new-folder", &["<Primary>n"]);
    app.set_accels_for_action("win.new-file", &["<Primary><Shift>n"]);
    app.set_accels_for_action("win.rename", &["F2"]);
    app.set_accels_for_action("win.copy", &["<Primary>c"]);
    app.set_accels_for_action("win.cut", &["<Primary>x"]);
    app.set_accels_for_action("win.paste", &["<Primary>v"]);
    app.set_accels_for_action("win.delete", &["Delete"]);
    app.set_accels_for_action("win.properties", &["<Alt>Return"]);
    app.set_accels_for_action("win.close", &["<Primary>q"]);
}
