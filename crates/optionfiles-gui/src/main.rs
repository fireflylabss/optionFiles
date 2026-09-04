//! `optionfiles-gtk` — GTK4 GUI for optionFiles.
//!
//! Bootstraps settings (`~/.option/files` via optionSDK) and opens the main
//! window. Like optionTerm, the app handles its own command line
//! (`HANDLES_COMMAND_LINE`): `PATH` opens a folder, `-a` shows hidden files.

mod actions;
mod app;
mod bridge;
mod crash;
mod places;
mod views;
mod watcher;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio::prelude::*;
use gtk::prelude::*;
use gtk::{gio, glib};
use libadwaita as adw;

use crate::app::AppWindow;

/// Pending launch: an optional folder plus an optional hidden-files override.
#[derive(Default)]
struct Launch {
    dir: Option<PathBuf>,
    show_hidden: Option<bool>,
}

fn parse_args(args: &[String]) -> Launch {
    let mut launch = Launch::default();
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "-a" | "--all" => launch.show_hidden = Some(true),
            "-h" | "--help" => {
                eprint_help();
                std::process::exit(0);
            }
            _ if launch.dir.is_none() => launch.dir = Some(PathBuf::from(arg)),
            _ => {}
        }
    }
    launch
}

fn eprint_help() {
    eprintln!("Usage: optionfiles-gtk [OPTIONS] [PATH]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -a, --all   Show hidden files");
    eprintln!("  -h, --help  Show this help");
}

fn main() -> glib::ExitCode {
    // Crash log first, before any GTK call: a later panic still lands in
    // `~/.option/files/crash.log`.
    crash::install();

    let application = adw::Application::builder()
        .application_id(option_sdk::App::FILES.bundle_id())
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    // Shared between `command-line` and the open window so a second instance
    // navigates the primary window instead of opening a new one.
    let pending: Rc<RefCell<Option<Launch>>> = Rc::new(RefCell::new(None));

    {
        let pending = pending.clone();
        application.connect_command_line(move |app, cmdline| {
            let args: Vec<String> = cmdline
                .arguments()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            *pending.borrow_mut() = Some(parse_args(&args));
            app.activate();
            0
        });
    }

    {
        let pending = pending.clone();
        application.connect_activate(move |app| {
            if let Err(e) = option_sdk::App::FILES.ensure() {
                eprintln!("cannot ensure ~/.option/files: {e}");
                return;
            }
            let launch = pending.borrow_mut().take().unwrap_or_default();
            let dir = launch.dir.unwrap_or_else(|| PathBuf::from("."));
            let show_hidden = launch.show_hidden.unwrap_or(false);

            if app.windows().is_empty() {
                let window = AppWindow::new(app, dir, show_hidden);
                window.show();
            } else {
                // A second launch reuses the window (folder navigation for a
                // path argument happens on next activation via pending state).
                if let Some(win) = app.active_window() {
                    win.present();
                }
            }
        });
    }

    application.run()
}
