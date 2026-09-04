//! Main window, Dolphin-style: narrow Places sidebar, topbar with back /
//! forward, centered breadcrumb pill, icon grid with system theme icons,
//! information panel and status bar.
//!
//! The `GuiState` owns the `optionfiles_core::App` (wrapped in `Option` so the
//! blocking `App::new` can run on a worker thread at startup). Every mutation
//! takes the core out, runs on a worker via `CoreBridge`, and puts it back —
//! the main thread never blocks on the filesystem. Sorting, filtering and
//! clipboard all run inside the core, so the GUI keeps full TUI parity.
//!
//! State lives in a `thread_local` because GTK callbacks require `Rc`
//! (non-`Send`); `AppWindow` keeps the window alive.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::gio::prelude::*;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use optionfiles_core::{App as CoreApp, SortMode};

use crate::bridge::CoreBridge;
use crate::places::{self, XdgDirs};
use crate::views::details::DetailsView;
use crate::views::dialogs;
use crate::views::grid::IconGrid;
use crate::views::pill::CrumbPill;
use crate::views::places_bar::{PlacesBar, SidebarData};
use crate::watcher::DirWatcher;

struct GuiState {
    core: Option<CoreApp>,
    view_icons: bool,
    show_sidebar: bool,
    syncing: bool,
    skip_record: bool,
    back_stack: Vec<PathBuf>,
    forward_stack: Vec<PathBuf>,
    favorites: Vec<PathBuf>,
    xdg: XdgDirs,
    window: adw::ApplicationWindow,
    back_btn: gtk::Button,
    forward_btn: gtk::Button,
    pill: Rc<CrumbPill>,
    star_btn: gtk::Button,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    view_btn: gtk::Button,
    sort_row: adw::ComboRow,
    hidden_row: adw::SwitchRow,
    status: gtk::Label,
    sidebar: Rc<PlacesBar>,
    split: adw::OverlaySplitView,
    stack: gtk::Stack,
    grid: Rc<IconGrid>,
    details: Rc<DetailsView>,
    toast: adw::ToastOverlay,
    _watcher: Option<DirWatcher>,
    _volume_monitor: gio::VolumeMonitor,
}

// Global state for the GTK callbacks (which use `Rc`, non-`Send`).
thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<GuiState>>>> = const { RefCell::new(None) };
}

fn state_rc() -> Rc<RefCell<GuiState>> {
    STATE.with(|s| s.borrow().clone().expect("GuiState not initialized"))
}

fn set_state_rc(rc: Rc<RefCell<GuiState>>) {
    STATE.with(|s| *s.borrow_mut() = Some(rc));
}

pub struct AppWindow {
    window: adw::ApplicationWindow,
}

impl AppWindow {
    pub fn new(app: &adw::Application, dir: PathBuf, show_hidden: bool) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("◆ optionFiles")
            .default_width(1120)
            .default_height(700)
            .build();

        // Dark like the reference screenshot; the menu offers overrides.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        if let Some(display) = gtk::gdk::Display::default() {
            install_css(&display);
        }

        // --- header: nav pill | breadcrumb pill | actions ---
        // Left to right, no overlaps: grouped back/forward, up, the wide
        // breadcrumb pill (title widget), then star, search, view and menu.
        let header = adw::HeaderBar::new();
        // Window controls follow `gtk-decoration-layout` (untouched, like optionTerm).
        header.set_show_start_title_buttons(true);
        header.set_show_end_title_buttons(true);

        let back_btn = gtk::Button::from_icon_name("go-previous-symbolic");
        back_btn.set_tooltip_text(Some("Back"));
        back_btn.set_action_name(Some("win.nav-back"));
        let forward_btn = gtk::Button::from_icon_name("go-next-symbolic");
        forward_btn.set_tooltip_text(Some("Forward"));
        forward_btn.set_action_name(Some("win.nav-forward"));
        let nav_pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        nav_pill.add_css_class("linked");
        nav_pill.add_css_class("option-navpill");
        nav_pill.append(&back_btn);
        nav_pill.append(&forward_btn);
        header.pack_start(&nav_pill);

        let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
        up_btn.set_tooltip_text(Some("Up one folder"));
        up_btn.set_action_name(Some("win.up"));
        header.pack_start(&up_btn);

        let pill = Rc::new(CrumbPill::new());
        header.set_title_widget(Some(&pill.root));

        let star_btn = gtk::Button::from_icon_name("starred-symbolic");
        star_btn.set_tooltip_text(Some("Toggle favorite for this folder"));
        star_btn.set_action_name(Some("win.favorite-toggle"));
        star_btn.add_css_class("flat");
        header.pack_end(&star_btn);

        let search_btn = gtk::Button::from_icon_name("system-search-symbolic");
        search_btn.set_tooltip_text(Some("Search (Ctrl+F)"));
        search_btn.set_action_name(Some("win.focus-search"));
        header.pack_end(&search_btn);

        let view_btn = gtk::Button::from_icon_name("view-grid-symbolic");
        view_btn.set_tooltip_text(Some("Toggle view"));
        view_btn.set_action_name(Some("win.view-toggle"));
        header.pack_end(&view_btn);

        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Menu")
            .menu_model(&build_menu())
            .build();
        header.pack_end(&menu_btn);

        // --- search bar (hidden until toggled) ---
        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some("Filter by name"));
        search_entry.set_hexpand(true);
        let search_bar = gtk::SearchBar::builder()
            .show_close_button(true)
            .child(&search_entry)
            .build();
        search_bar.connect_entry(&search_entry);
        {
            let bar_c = search_bar.clone();
            search_entry.connect_stop_search(move |_| {
                bar_c.set_search_mode(false);
            });
        }

        // --- sidebar + browser (full width) ---
        let sidebar = Rc::new(PlacesBar::new());
        let grid = Rc::new(IconGrid::new());
        let details = Rc::new(DetailsView::new());

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&grid.root, Some("icons"));
        stack.add_named(&details.root, Some("details"));
        stack.set_visible_child_name("icons");

        let split = adw::OverlaySplitView::new();
        split.set_sidebar(Some(&sidebar.root));
        split.set_content(Some(&stack));
        split.set_sidebar_width_fraction(0.18);
        split.set_min_sidebar_width(170.0);
        split.set_max_sidebar_width(240.0);
        split.set_show_sidebar(true);

        // --- view options popover: sort + hidden + info rows ---
        let sort_row = adw::ComboRow::builder()
            .title("Sort")
            .subtitle("Order files by")
            .model(&gtk::StringList::new(&["Name", "Size", "Date"]))
            .build();
        let hidden_row = adw::SwitchRow::builder()
            .title("Show hidden")
            .subtitle("Show dotfiles")
            .build();
        let options_list = gtk::ListBox::new();
        options_list.set_selection_mode(gtk::SelectionMode::None);
        options_list.add_css_class("boxed-list");
        options_list.append(&sort_row);
        options_list.append(&hidden_row);
        options_list.set_margin_top(6);
        options_list.set_margin_bottom(6);
        options_list.set_margin_start(6);
        options_list.set_margin_end(6);
        let options_popover = gtk::Popover::new();
        options_popover.set_child(Some(&options_list));
        // The `…` button lives inside the breadcrumb pill (right edge).
        pill.set_options_popover(&options_popover);

        let status = gtk::Label::new(Some("Loading…"));
        status.set_halign(gtk::Align::Start);
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.set_margin_start(12);
        status.set_margin_end(12);
        status.set_margin_top(4);
        status.set_margin_bottom(6);
        status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        status.set_single_line_mode(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&search_bar);
        content.append(&split);
        content.append(&status);

        let toast = adw::ToastOverlay::new();
        toast.set_child(Some(&content));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toast));
        window.set_content(Some(&toolbar));

        crate::actions::register(&window, app);

        let volume_monitor = gio::VolumeMonitor::get();
        {
            let monitor = volume_monitor.clone();
            monitor.connect_mount_added(|_, _| rebuild_sidebar());
            let monitor = volume_monitor.clone();
            monitor.connect_mount_removed(|_, _| rebuild_sidebar());
            let monitor = volume_monitor.clone();
            monitor.connect_mount_changed(|_, _| rebuild_sidebar());
            let monitor = volume_monitor.clone();
            monitor.connect_volume_added(|_, _| rebuild_sidebar());
            monitor.connect_volume_removed(|_, _| rebuild_sidebar());
        }

        let state = Rc::new(RefCell::new(GuiState {
            core: None,
            view_icons: true,
            show_sidebar: true,
            syncing: false,
            skip_record: false,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            favorites: Vec::new(),
            xdg: XdgDirs::fallback(),
            window: window.clone(),
            back_btn: back_btn.clone(),
            forward_btn: forward_btn.clone(),
            pill: pill.clone(),
            star_btn: star_btn.clone(),
            search_bar: search_bar.clone(),
            search_entry: search_entry.clone(),
            view_btn: view_btn.clone(),
            sort_row: sort_row.clone(),
            hidden_row: hidden_row.clone(),
            status,
            sidebar: sidebar.clone(),
            split: split.clone(),
            stack: stack.clone(),
            grid: grid.clone(),
            details: details.clone(),
            toast,
            _watcher: None,
            _volume_monitor: volume_monitor,
        }));
        set_state_rc(state.clone());

        // --- signals ---
        let on_select = |index: usize| {
            select_row(index);
        };
        grid.on_select(on_select);
        details.on_select(|index| {
            select_row(index);
        });

        grid.on_activate(|index| {
            activate_index(index);
        });
        details.on_activate(|index| {
            activate_index(index);
        });

        search_entry.connect_search_changed(|entry| {
            let text = entry.text().to_string();
            let st = state_rc();
            if st.borrow().syncing {
                return;
            }
            let same = st
                .borrow()
                .core
                .as_ref()
                .map(|a| a.filter == text)
                .unwrap_or(true);
            if same {
                return;
            }
            drop(st);
            mutate(move |core| {
                core.set_filter(text)?;
                Ok(())
            });
        });

        sort_row.connect_selected_notify(|row| {
            let st = state_rc();
            if st.borrow().syncing {
                return;
            }
            let mode = match row.selected() {
                1 => SortMode::Size,
                2 => SortMode::Modified,
                _ => SortMode::Name,
            };
            drop(st);
            mutate(move |core| {
                core.sort = mode;
                core.refresh()?;
                Ok(())
            });
        });

        hidden_row.connect_active_notify(|row| {
            let st = state_rc();
            if st.borrow().syncing {
                return;
            }
            let on = row.is_active();
            drop(st);
            mutate(move |core| {
                core.show_hidden = on;
                core.refresh()?;
                Ok(())
            });
        });

        install_window_keys(&window, &search_entry, &search_bar);

        // Blocking open happens on a worker thread, never on the main thread.
        init_core(dir, show_hidden);

        Self { window }
    }

    pub fn show(&self) {
        self.window.present();
    }
}

/// Hamburger menu model (actions are registered in `actions.rs`).
fn build_menu() -> gio::Menu {
    let menu = gio::Menu::new();

    let file = gio::Menu::new();
    file.append(Some("New Folder"), Some("win.new-folder"));
    file.append(Some("New File"), Some("win.new-file"));
    file.append(Some("Rename"), Some("win.rename"));
    file.append(Some("Move to Trash"), Some("win.delete"));
    file.append(Some("Properties"), Some("win.properties"));
    file.append(Some("Toggle Favorite"), Some("win.favorite-toggle"));
    menu.append_section(Some("File"), &file);

    let edit = gio::Menu::new();
    edit.append(Some("Copy"), Some("win.copy"));
    edit.append(Some("Cut"), Some("win.cut"));
    edit.append(Some("Paste"), Some("win.paste"));
    menu.append_section(Some("Edit"), &edit);

    let view = gio::Menu::new();
    view.append(Some("Icons"), Some("win.view-icons"));
    view.append(Some("Details"), Some("win.view-details"));
    view.append(Some("Toggle Sidebar"), Some("win.toggle-sidebar"));
    view.append(Some("Show Hidden Files"), Some("win.toggle-hidden"));
    view.append(Some("Up One Folder"), Some("win.up"));
    menu.append_section(Some("View"), &view);

    let go = gio::Menu::new();
    go.append(Some("Open"), Some("win.open"));
    go.append(Some("Edit with $EDITOR"), Some("win.edit"));
    go.append(Some("Search"), Some("win.focus-search"));
    menu.append_section(Some("Go"), &go);

    let theme = gio::Menu::new();
    theme.append(Some("Dark"), Some("win.theme-dark"));
    theme.append(Some("Follow System"), Some("win.theme-system"));
    theme.append(Some("Light"), Some("win.theme-light"));
    menu.append_submenu(Some("Theme"), &theme);

    let help = gio::Menu::new();
    help.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    help.append(Some("About optionFiles"), Some("win.about"));
    menu.append_section(None, &help);

    let quit = gio::Menu::new();
    quit.append(Some("Quit"), Some("win.close"));
    menu.append_section(None, &quit);

    menu
}

/// TUI-parity keyboard handling: plain keys act on the browser when the
/// filter box is not focused (entries consume their own keys, so typing
/// never triggers actions).
fn install_window_keys(
    window: &adw::ApplicationWindow,
    search_entry: &gtk::SearchEntry,
    search_bar: &gtk::SearchBar,
) {
    let search_c = search_entry.clone();
    let bar_c = search_bar.clone();
    let window_c = window.clone();
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, keyval, _, modifier| {
        if modifier.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            || modifier.contains(gtk::gdk::ModifierType::ALT_MASK)
            || modifier.contains(gtk::gdk::ModifierType::SUPER_MASK)
        {
            return glib::Propagation::Proceed;
        }
        let in_search = search_c.has_focus();
        match keyval {
            gtk::gdk::Key::Escape => {
                // Closes the search without stealing grid focus; anywhere
                // else Esc does nothing (Q / Ctrl+Q closes the window).
                if !in_search {
                    return glib::Propagation::Proceed;
                }
                if !search_c.text().is_empty() {
                    search_c.set_text("");
                } else {
                    bar_c.set_search_mode(false);
                    focus_browser();
                }
                return glib::Propagation::Stop;
            }
            _ if in_search => return glib::Propagation::Proceed,
            _ => {}
        }
        // Focus inside the browser: Enter keeps its native meaning (activate
        // the row), so only plain letters/digits are remapped here.
        let st = state_rc();
        let in_browser = focus_within(&window_c, &st.borrow().stack);
        let icons_mode = st.borrow().view_icons;
        drop(st);
        // Grid step: one row up/down in the icon grid, one item in details.
        let row_step = if icons_mode {
            state_rc().borrow().grid.columns().max(1) as isize
        } else {
            1
        };
        let handled = match keyval {
            gtk::gdk::Key::j => {
                move_selection(row_step);
                true
            }
            gtk::gdk::Key::k => {
                move_selection(-row_step);
                true
            }
            gtk::gdk::Key::h => {
                go_up();
                true
            }
            gtk::gdk::Key::l => {
                activate_current();
                true
            }
            gtk::gdk::Key::g => {
                select_first();
                true
            }
            gtk::gdk::Key::G => {
                select_last();
                true
            }
            gtk::gdk::Key::a | gtk::gdk::Key::period => {
                toggle_hidden();
                true
            }
            gtk::gdk::Key::s => {
                cycle_sort();
                true
            }
            gtk::gdk::Key::c => {
                copy_selected();
                true
            }
            gtk::gdk::Key::x => {
                cut_selected();
                true
            }
            gtk::gdk::Key::v => {
                paste_here();
                true
            }
            gtk::gdk::Key::n => {
                show_new_folder();
                true
            }
            gtk::gdk::Key::N => {
                show_new_file();
                true
            }
            gtk::gdk::Key::r => {
                show_rename();
                true
            }
            gtk::gdk::Key::d => {
                show_delete();
                true
            }
            gtk::gdk::Key::e => {
                edit_selected();
                true
            }
            gtk::gdk::Key::o => {
                open_selected();
                true
            }
            gtk::gdk::Key::q => {
                close_window();
                true
            }
            gtk::gdk::Key::question => {
                show_shortcuts();
                true
            }
            gtk::gdk::Key::slash => {
                focus_search();
                true
            }
            // Space is intentionally unbound: there is no preview panel
            // to toggle, and activating rows stays on Enter/double-click.
            gtk::gdk::Key::Return if !in_browser => {
                activate_current();
                true
            }
            gtk::gdk::Key::Home => {
                select_first();
                true
            }
            gtk::gdk::Key::End => {
                select_last();
                true
            }
            gtk::gdk::Key::Page_Up => {
                move_selection(-row_step * 3);
                true
            }
            gtk::gdk::Key::Page_Down => {
                move_selection(row_step * 3);
                true
            }
            _ => false,
        };
        if handled {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
}

/// Minimal stylesheet, installed once per display. Icons come from the icon
/// theme (untouched); CSS only paints the dark Dolphin-style background,
/// dims dotfiles, rounds the pill and marks the current place.
fn install_css(display: &gtk::gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
window { background-color: #212121; }
.option-grid { background-color: #212121; }
.option-grid listview > * { padding: 10px 4px; }
.option-grid-label { font-size: 12px; }
.option-sidebar { background-color: #1e1e1e; }
.option-places { background-color: transparent; }
.option-places row { border-radius: 8px; padding-top: 1px; padding-bottom: 1px; }
.option-place-label { font-size: 13px; }
.option-place-icon { opacity: 0.9; }
.option-place-current { background-color: alpha(@theme_fg_color, 0.12); }
.option-place-current .option-place-label { color: #f0a35e; font-weight: 600; }
.option-pill { background-color: alpha(@theme_fg_color, 0.08); border-radius: 999px; padding: 2px 6px 2px 14px; }
.option-pill label { font-weight: 600; }
.option-navpill { border-radius: 999px; }
.option-path-footer { color: alpha(@theme_fg_color, 0.45); font-size: 11px; }
.starred-off { opacity: 0.45; }
.option-view-active { background-color: alpha(@theme_fg_color, 0.16); }
"#,
    );
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

// --- core plumbing ---

/// Builds the core `App` off the main thread, then paints and loads places.
fn init_core(dir: PathBuf, show_hidden: bool) {
    toast("Loading…");
    CoreBridge::call(
        move || CoreApp::new(&dir, show_hidden),
        move |res: anyhow::Result<CoreApp>| match res {
            Ok(core) => {
                let st = state_rc();
                st.borrow_mut().core = Some(core);
                rewatch();
                repaint();
                load_places();
            }
            Err(e) => {
                toast(&format!("Cannot open folder: {e}"));
                load_places();
                repaint();
            }
        },
    );
}

/// Loads favorites and localized folders off the main thread.
fn load_places() {
    CoreBridge::call(
        || (places::load_favorites(), places::resolve_xdg()),
        |(favorites, xdg): (Vec<PathBuf>, XdgDirs)| {
            let st = state_rc();
            st.borrow_mut().favorites = favorites;
            st.borrow_mut().xdg = xdg;
            rebuild_sidebar();
            update_star();
        },
    );
}

/// Runs `op` on the core in a worker thread, then repaints.
/// The core is taken out of the state while the worker owns it, so navigation
/// fields (`previous_dir`, clipboard) survive across operations. A folder
/// change records history unless `record_history` is false
/// (back/forward/previous manage their own stacks).
fn mutate_with(
    op: impl FnOnce(&mut CoreApp) -> anyhow::Result<()> + Send + 'static,
    record_history: bool,
) {
    let st = state_rc();
    if !record_history {
        st.borrow_mut().skip_record = true;
    }
    let old_cwd = st.borrow().core.as_ref().map(|a| a.cwd.clone());
    let Some(core) = st.borrow_mut().core.take() else {
        toast("Loading…");
        return;
    };
    CoreBridge::call(
        move || {
            let mut core = core;
            let res = op(&mut core);
            (core, res)
        },
        move |(core, res): (CoreApp, anyhow::Result<()>)| {
            let st = state_rc();
            let cwd_changed = old_cwd.as_ref() != Some(&core.cwd);
            if let Err(e) = res {
                // Never crash on a bad folder: toast the error, repaint the
                // last good state, and fall back home when the folder itself
                // is gone (deleted / permission revoked). The existence check
                // runs on a worker; `go_home` only toasts on its own failure,
                // so this cannot loop.
                let gone_cwd = core.cwd.clone();
                st.borrow_mut().core = Some(core);
                st.borrow_mut().skip_record = false;
                repaint();
                toast(&e.to_string());
                CoreBridge::call(
                    move || !gone_cwd.is_dir(),
                    move |gone: bool| {
                        if gone {
                            go_home();
                        }
                    },
                );
                return;
            }
            st.borrow_mut().core = Some(core);
            if cwd_changed {
                {
                    let mut st = st.borrow_mut();
                    if st.skip_record {
                        st.skip_record = false;
                    } else {
                        if let Some(old) = old_cwd {
                            st.back_stack.push(old);
                        }
                        st.forward_stack.clear();
                    }
                }
                // The watcher follows the folder; drop the old watch first.
                rewatch();
            }
            repaint();
        },
    );
}

/// Plain mutation with history recording (filter, sort, CRUD, navigation).
fn mutate(op: impl FnOnce(&mut CoreApp) -> anyhow::Result<()> + Send + 'static) {
    mutate_with(op, true);
}

/// Plain refresh (watcher, F5). Silent — keeps the core status line.
pub fn refresh() {
    let st = state_rc();
    if st.borrow().core.is_none() {
        return;
    }
    drop(st);
    mutate(|core| {
        core.refresh()?;
        Ok(())
    });
}

/// Manual refresh with confirmation in the status line.
pub fn refresh_manual() {
    let st = state_rc();
    if st.borrow().core.is_none() {
        return;
    }
    drop(st);
    mutate(|core| {
        core.refresh()?;
        core.status = "refreshed".into();
        Ok(())
    });
}

/// Re-creates the directory watcher for the current folder.
fn rewatch() {
    let st = state_rc();
    let cwd = match st.borrow().core.as_ref() {
        Some(core) => core.cwd.clone(),
        None => return,
    };
    st.borrow_mut()._watcher = None;
    match DirWatcher::watch(&cwd, Duration::from_millis(250), refresh) {
        Ok(watcher) => {
            st.borrow_mut()._watcher = Some(watcher);
        }
        Err(e) => {
            toast(&format!("Watcher off: {e}"));
        }
    }
}

/// Plain data copied out of the core for painting (the core itself is not
/// `Clone`, and widgets must update without holding its borrow).
struct Snapshot {
    cwd: PathBuf,
    sort: SortMode,
    show_hidden: bool,
    entries: Vec<optionfiles_core::Entry>,
    selected: usize,
    status: String,
    filter: String,
    clipboard: Option<optionfiles_core::Clipboard>,
}

/// Sort label in the status bar.
fn sort_label(sort: SortMode) -> &'static str {
    match sort {
        SortMode::Name => "nome",
        SortMode::Size => "tamanho",
        SortMode::Modified => "data",
    }
}

/// Repaints pill, views, panel, sidebar markers and status.
fn repaint() {
    let snapshot = {
        let st = state_rc();
        let borrowed = st.borrow();
        let core = match borrowed.core.as_ref() {
            Some(core) => core,
            None => return,
        };
        Snapshot {
            cwd: core.cwd.clone(),
            sort: core.sort,
            show_hidden: core.show_hidden,
            entries: core.entries.clone(),
            selected: core.selected,
            status: core.status.clone(),
            filter: core.filter.clone(),
            clipboard: core.clipboard.clone(),
        }
    };
    let snapshot_cwd = snapshot.cwd.clone();

    let st = state_rc();
    // Programmatic widget updates must not re-trigger their own signals.
    st.borrow_mut().syncing = true;
    st.borrow().split.set_show_sidebar(st.borrow().show_sidebar);
    let icons_mode = st.borrow().view_icons;
    st.borrow()
        .stack
        .set_visible_child_name(if icons_mode { "icons" } else { "details" });
    st.borrow().view_btn.set_icon_name(if icons_mode {
        "view-grid-symbolic"
    } else {
        "view-list-symbolic"
    });
    st.borrow()
        .back_btn
        .set_sensitive(!st.borrow().back_stack.is_empty());
    st.borrow()
        .forward_btn
        .set_sensitive(!st.borrow().forward_stack.is_empty());
    st.borrow().pill.set_path(&snapshot_cwd, |path| {
        navigate_to(path);
    });
    st.borrow().sort_row.set_selected(match snapshot.sort {
        SortMode::Name => 0,
        SortMode::Size => 1,
        SortMode::Modified => 2,
    });
    st.borrow().hidden_row.set_active(snapshot.show_hidden);
    st.borrow().status.set_text(&status_line(&snapshot));
    if icons_mode {
        st.borrow()
            .grid
            .set_entries(&snapshot.entries, snapshot.selected);
    } else {
        st.borrow()
            .details
            .set_entries(&snapshot.entries, snapshot.selected);
    }
    // Keep keyboard focus in the browser across rebuilds, but never steal it
    // from the filter box while the user is typing.
    if focus_within(&st.borrow().window, &st.borrow().stack) {
        focus_browser_at(snapshot.selected);
    }
    st.borrow_mut().syncing = false;
    drop(st);

    rebuild_sidebar();
    update_star();
}

/// Rebuilds the Places sidebar (current-folder marker, favorites).
fn rebuild_sidebar() {
    let st = state_rc();
    let (data, has_core) = {
        let st = st.borrow();
        let data = SidebarData {
            cwd: st
                .core
                .as_ref()
                .map(|a| a.cwd.clone())
                .unwrap_or_else(|| PathBuf::from("/")),
            favorites: st.favorites.clone(),
            xdg: st.xdg.clone(),
        };
        (data, st.core.is_some())
    };
    if !has_core {
        return;
    }
    st.borrow().sidebar.rebuild(
        &data,
        |path| navigate_to(path),
        || toast("Network browsing is not available"),
        |index| remove_favorite(index),
    );
}

/// Updates the favorite star for the current folder.
fn update_star() {
    let st = state_rc();
    let active = {
        let st = st.borrow();
        st.core
            .as_ref()
            .map(|a| st.favorites.contains(&a.cwd))
            .unwrap_or(false)
    };
    if active {
        st.borrow().star_btn.remove_css_class("starred-off");
    } else {
        st.borrow().star_btn.add_css_class("starred-off");
    }
}

/// Dolphin-style footer: item count, sort order, filter, clipboard, status.
fn status_line(snapshot: &Snapshot) -> String {
    let mut parts = vec![format!(
        "{} {}",
        snapshot.entries.len(),
        if snapshot.entries.len() == 1 {
            "item"
        } else {
            "itens"
        }
    )];
    parts.push(format!("ordem {}", sort_label(snapshot.sort)));
    if !snapshot.filter.is_empty() {
        parts.push(format!("filtro '{}'", snapshot.filter));
    }
    if let Some(clip) = &snapshot.clipboard {
        let verb = match clip.mode {
            optionfiles_core::ClipboardMode::Copy => "copiar",
            optionfiles_core::ClipboardMode::Cut => "recortar",
        };
        parts.push(format!(
            "{verb} {}",
            clip.path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    if !snapshot.status.is_empty() {
        parts.push(snapshot.status.clone());
    }
    parts.join(" · ")
}

/// Whether keyboard focus is currently inside `container`.
fn focus_within(window: &adw::ApplicationWindow, container: &impl IsA<gtk::Widget>) -> bool {
    let Some(focused) = gtk::prelude::GtkWindowExt::focus(window) else {
        return false;
    };
    let root_ptr = container.as_ptr() as *const std::ffi::c_void;
    let mut current: Option<gtk::Widget> = Some(focused);
    while let Some(widget) = current {
        if widget.as_ptr() as *const std::ffi::c_void == root_ptr {
            return true;
        }
        current = widget.parent();
    }
    false
}

/// Moves focus into the browser at `index` (used after clearing the filter).
fn focus_browser_at(index: usize) {
    let st = state_rc();
    if st.borrow().view_icons {
        st.borrow().grid.focus();
    } else {
        st.borrow().details.focus();
    }
    drop(st);
    select_visual(index);
}

/// Selects `index` in the visible view only (no core change).
fn select_visual(index: usize) {
    let st = state_rc();
    if st.borrow().view_icons {
        st.borrow().grid.select_index(index);
    } else {
        st.borrow().details.select_index(index);
    }
}

/// Moves focus into the browser at the current selection.
fn focus_browser() {
    let selected = state_rc()
        .borrow()
        .core
        .as_ref()
        .map(|a| a.selected)
        .unwrap_or(0);
    focus_browser_at(selected);
}

/// Short transient message.
pub fn toast(msg: &str) {
    let st = state_rc();
    st.borrow()
        .toast
        .add_toast(adw::Toast::builder().title(msg).timeout(2).build());
}

// --- navigation ---

/// Navigates to an explicit folder (sidebar, pill, history).
/// Paths come from the UI or history and are already absolute; the worker
/// normalizes them so symlinked folders resolve like the TUI `enter`.
pub fn navigate_to(path: PathBuf) {
    mutate(move |core| {
        let target = path.canonicalize().unwrap_or(path);
        if target == core.cwd {
            return Ok(());
        }
        core.previous_dir = Some(core.cwd.clone());
        core.cwd = target;
        core.selected = 0;
        core.scroll = 0;
        core.refresh()?;
        Ok(())
    });
}

/// History-aware raw navigation (back/forward): no new history recorded.
fn navigate_raw(path: PathBuf) {
    mutate_with(
        move |core| {
            let target = path.canonicalize().unwrap_or(path);
            core.previous_dir = Some(core.cwd.clone());
            core.cwd = target;
            core.selected = 0;
            core.scroll = 0;
            core.refresh()?;
            Ok(())
        },
        false,
    );
}

/// Selects a row from view callbacks. Validates the index and toasts
/// instead of panicking when the core is not ready yet.
fn select_row(index: usize) {
    let st = state_rc();
    if st.borrow().syncing {
        return;
    }
    let valid = st
        .borrow()
        .core
        .as_ref()
        .map(|a| index < a.entries.len())
        .unwrap_or(false);
    if !valid {
        return;
    }
    let ready = match st.borrow_mut().core.as_mut() {
        Some(core) => {
            core.selected = index;
            true
        }
        None => false,
    };
    if !ready {
        toast("Loading…");
    }
}

fn activate_index(index: usize) {
    let st = state_rc();
    let entry = match st
        .borrow()
        .core
        .as_ref()
        .and_then(|a| a.entries.get(index).cloned())
    {
        Some(entry) => entry,
        None => return,
    };
    match st.borrow_mut().core.as_mut() {
        Some(core) => core.selected = index,
        None => {
            toast("Loading…");
            return;
        }
    }
    drop(st);
    if entry.is_dir {
        mutate(move |core| {
            core.enter()?;
            Ok(())
        });
    } else {
        open_path(&entry.path);
    }
}

/// Opens the current row (enter on a folder, default app on a file).
pub fn activate_current() {
    let index = state_rc()
        .borrow()
        .core
        .as_ref()
        .map(|a| a.selected)
        .unwrap_or(0);
    activate_index(index);
}

/// Moves the selection by `delta` rows (pure index math, no IO).
pub fn move_selection(delta: isize) {
    let selected = {
        let rc = state_rc();
        let mut st = rc.borrow_mut();
        let Some(core) = st.core.as_mut() else {
            return;
        };
        if core.entries.is_empty() {
            return;
        }
        core.move_by(delta);
        core.selected
    };
    select_visual(selected);
}

pub fn select_first() {
    let rc = state_rc();
    let mut st = rc.borrow_mut();
    let Some(core) = st.core.as_mut() else {
        return;
    };
    core.home();
    drop(st);
    focus_browser();
}

pub fn select_last() {
    let rc = state_rc();
    let mut st = rc.borrow_mut();
    let Some(core) = st.core.as_mut() else {
        return;
    };
    core.end();
    drop(st);
    focus_browser();
}

/// TUI `backspace`: spatial back through visited folders.
pub fn go_history_back() {
    let target = {
        let rc = state_rc();
        let mut st = rc.borrow_mut();
        let Some(target) = st.back_stack.pop() else {
            return;
        };
        if let Some(cwd) = st.core.as_ref().map(|a| a.cwd.clone()) {
            st.forward_stack.push(cwd);
        }
        target
    };
    navigate_raw(target);
}

/// Spatial forward through visited folders.
pub fn go_history_forward() {
    let target = {
        let rc = state_rc();
        let mut st = rc.borrow_mut();
        let Some(target) = st.forward_stack.pop() else {
            return;
        };
        if let Some(cwd) = st.core.as_ref().map(|a| a.cwd.clone()) {
            st.back_stack.push(cwd);
        }
        target
    };
    navigate_raw(target);
}

/// TUI `-`: toggles with the previous location (no history recorded).
pub fn go_previous() {
    mutate_with(
        |core| {
            core.go_previous()?;
            Ok(())
        },
        false,
    );
}

pub fn go_home() {
    mutate(|core| {
        core.go_home()?;
        Ok(())
    });
}

pub fn go_up() {
    mutate(|core| {
        core.parent()?;
        Ok(())
    });
}

// --- view modes / toggles ---

pub fn set_view_icons(icons_mode: bool) {
    let st = state_rc();
    st.borrow_mut().view_icons = icons_mode;
    drop(st);
    // View switches only swap the visible child; the model is already current.
    let st = state_rc();
    st.borrow_mut().syncing = true;
    let selected = st.borrow().core.as_ref().map(|a| a.selected).unwrap_or(0);
    st.borrow()
        .stack
        .set_visible_child_name(if icons_mode { "icons" } else { "details" });
    st.borrow().view_btn.set_icon_name(if icons_mode {
        "view-grid-symbolic"
    } else {
        "view-list-symbolic"
    });
    if icons_mode {
        st.borrow().grid.set_entries(
            &st.borrow()
                .core
                .as_ref()
                .map(|a| a.entries.clone())
                .unwrap_or_default(),
            selected,
        );
    } else {
        st.borrow().details.set_entries(
            &st.borrow()
                .core
                .as_ref()
                .map(|a| a.entries.clone())
                .unwrap_or_default(),
            selected,
        );
    }
    st.borrow_mut().syncing = false;
}

/// Toggles between the icon grid and the details columns.
pub fn toggle_view() {
    set_view_icons(!state_rc().borrow().view_icons);
}

/// TUI `s`: cycles name → size → date inside the core.
pub fn cycle_sort() {
    mutate(|core| {
        core.cycle_sort()?;
        Ok(())
    });
}

/// TUI `a`/`.` — flips hidden files inside the core.
pub fn toggle_hidden() {
    mutate(|core| {
        core.toggle_hidden()?;
        Ok(())
    });
}

pub fn toggle_sidebar() {
    let st = state_rc();
    let visible = !st.borrow().show_sidebar;
    st.borrow_mut().show_sidebar = visible;
    st.borrow().split.set_show_sidebar(visible);
}

// --- favorites ---

/// Adds the current folder to favorites, or removes it when present.
pub fn toggle_favorite() {
    let cwd = match state_rc().borrow().core.as_ref().map(|a| a.cwd.clone()) {
        Some(cwd) => cwd,
        None => return,
    };
    {
        let rc = state_rc();
        let mut st = rc.borrow_mut();
        if st.favorites.contains(&cwd) {
            st.favorites.retain(|p| p != &cwd);
        } else {
            st.favorites.push(cwd);
        }
    }
    persist_favorites();
    rebuild_sidebar();
    update_star();
}

fn remove_favorite(index: usize) {
    {
        let rc = state_rc();
        let mut st = rc.borrow_mut();
        if index < st.favorites.len() {
            st.favorites.remove(index);
        }
    }
    persist_favorites();
    rebuild_sidebar();
    update_star();
}

/// Saves favorites off the main thread (fire-and-forget).
fn persist_favorites() {
    let favorites = state_rc().borrow().favorites.clone();
    CoreBridge::call(
        move || places::save_favorites(&favorites),
        |res: std::io::Result<()>| {
            if let Err(e) = res {
                toast(&format!("Cannot save favorites: {e}"));
            }
        },
    );
}

// --- clipboard (core copy/cut/paste + wl-copy export) ---

pub fn copy_selected() {
    mutate(|core| {
        core.set_clipboard(optionfiles_core::ClipboardMode::Copy);
        Ok(())
    });
}

pub fn cut_selected() {
    mutate(|core| {
        core.set_clipboard(optionfiles_core::ClipboardMode::Cut);
        Ok(())
    });
}

pub fn paste_here() {
    mutate(|core| {
        core.paste()?;
        Ok(())
    });
}

// --- open / edit / properties ---

fn uri_for(path: &std::path::Path) -> String {
    gio::File::for_path(path).uri().to_string()
}

/// Opens a path with the default application (`o` in the TUI).
fn open_path(path: &std::path::Path) {
    let uri = uri_for(path);
    if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
        toast(&format!("Cannot open: {e}"));
    }
}

pub fn open_selected() {
    let st = state_rc();
    let path = st
        .borrow()
        .core
        .as_ref()
        .and_then(|a| a.current().map(|e| e.path.clone()));
    drop(st);
    if let Some(path) = path {
        open_path(&path);
    }
}

/// Edits the current file with $EDITOR (`e` in the TUI).
pub fn edit_selected() {
    let st = state_rc();
    let path = st
        .borrow()
        .core
        .as_ref()
        .and_then(|a| a.current().map(|e| e.path.clone()));
    drop(st);
    let Some(path) = path else { return };
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if editor.is_empty() {
        toast("Set $EDITOR to edit files");
        return;
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    match std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("optionfiles-editor")
        .arg(&path)
        .spawn()
    {
        Ok(_) => toast(&format!("Editing {name}")),
        Err(e) => toast(&format!("Cannot edit: {e}")),
    }
}

/// Properties dialog mirroring the CLI `info` command. Permission and owner
/// strings follow the CLI formatting; the metadata read runs on a worker.
pub fn show_properties() {
    let entry = state_rc()
        .borrow()
        .core
        .as_ref()
        .and_then(|a| a.current().cloned());
    let Some(entry) = entry else {
        toast("Nothing selected");
        return;
    };
    CoreBridge::call(
        move || {
            let (mode, owner) = permission_strings(&entry.path);
            let kind = if entry.is_dir {
                "directory".to_string()
            } else if entry.is_symlink {
                "symbolic link".to_string()
            } else {
                "file".to_string()
            };
            let size = if entry.is_dir {
                "—".to_string()
            } else {
                optionfiles_core::human_size(entry.size)
            };
            let modified = entry
                .modified
                .map(optionfiles_core::format_time)
                .unwrap_or_else(|| "unknown".into());
            (
                entry.name,
                kind,
                size,
                entry.path.display().to_string(),
                mode,
                owner,
                modified,
            )
        },
        |(name, kind, size, path, mode, owner, modified)| {
            let st = state_rc();
            dialogs::show_properties(
                &st.borrow().window,
                &name,
                &kind,
                &size,
                &path,
                &mode,
                &owner,
                &modified,
            );
        },
    );
}

/// Permission + owner strings in the CLI `info` style (`-rw-r--r--`,
/// `uid 1000 · gid 100`).
fn permission_strings(path: &std::path::Path) -> (String, String) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return ("—".into(), "—".into());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            'd'
        } else if file_type.is_symlink() {
            'l'
        } else {
            '-'
        };
        let bits = [
            (0o400, 'r'),
            (0o200, 'w'),
            (0o100, 'x'),
            (0o040, 'r'),
            (0o020, 'w'),
            (0o010, 'x'),
            (0o004, 'r'),
            (0o002, 'w'),
            (0o001, 'x'),
        ];
        let perms: String = bits
            .iter()
            .map(|(bit, c)| if mode & bit != 0 { *c } else { '-' })
            .collect();
        (
            format!("{kind}{perms}"),
            format!("uid {} · gid {}", metadata.uid(), metadata.gid()),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        ("—".into(), "—".into())
    }
}

// --- create / rename / delete (dialogs + core ops) ---

pub fn show_new_folder() {
    let st = state_rc();
    let window = st.borrow().window.clone();
    drop(st);
    dialogs::prompt_name(
        &window,
        "New folder",
        "Name for the new folder",
        "",
        "Create",
        |name| {
            if name.is_empty() {
                toast("Name is empty");
                return;
            }
            mutate(move |core| {
                core.create_dir(&name)?;
                Ok(())
            });
        },
    );
}

pub fn show_new_file() {
    let st = state_rc();
    let window = st.borrow().window.clone();
    drop(st);
    dialogs::prompt_name(
        &window,
        "New file",
        "Name for the new file",
        "",
        "Create",
        |name| {
            if name.is_empty() {
                toast("Name is empty");
                return;
            }
            mutate(move |core| {
                core.create_file(&name)?;
                Ok(())
            });
        },
    );
}

pub fn show_rename() {
    let st = state_rc();
    let (window, current) = (
        st.borrow().window.clone(),
        st.borrow()
            .core
            .as_ref()
            .and_then(|a| a.current().map(|e| e.name.clone())),
    );
    drop(st);
    let Some(current) = current else {
        toast("Nothing selected");
        return;
    };
    dialogs::prompt_name(&window, "Rename", "New name", &current, "Rename", |name| {
        if name.is_empty() {
            toast("Name is empty");
            return;
        }
        mutate(move |core| {
            core.rename_current(&name)?;
            Ok(())
        });
    });
}

pub fn show_delete() {
    let st = state_rc();
    let (window, current) = (
        st.borrow().window.clone(),
        st.borrow()
            .core
            .as_ref()
            .and_then(|a| a.current().map(|e| e.name.clone())),
    );
    drop(st);
    let Some(current) = current else {
        toast("Nothing selected");
        return;
    };
    dialogs::confirm_delete(&window, &current, || {
        mutate(|core| {
            core.delete_current()?;
            Ok(())
        });
    });
}

// --- search focus / theme / help / close ---

pub fn focus_search() {
    let st = state_rc();
    st.borrow().search_bar.set_search_mode(true);
    st.borrow().search_entry.grab_focus();
}

pub fn set_theme(scheme: adw::ColorScheme) {
    adw::StyleManager::default().set_color_scheme(scheme);
}

pub fn show_shortcuts() {
    let st = state_rc();
    dialogs::show_shortcuts(&st.borrow().window);
}

pub fn show_about() {
    let st = state_rc();
    dialogs::show_about(&st.borrow().window);
}

pub fn close_window() {
    let st = state_rc();
    st.borrow().window.close();
}
