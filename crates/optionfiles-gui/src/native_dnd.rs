use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use optionfiles_core::{
    Clipboard, ClipboardMode, clipboard_decode, clipboard_encode, parse_file_uri, path_uri,
};

const MAX_URI_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 65536;

struct Session {
    main_loop: glib::MainLoop,
    done: Cell<bool>,
    exit_code: Cell<i32>,
    pending_drop: RefCell<Option<gdk::Drop>>,
    drop_ready: Cell<bool>,
}

impl Session {
    fn event(&self, line: &str) -> bool {
        let mut stdout = io::stdout().lock();
        if writeln!(stdout, "{line}")
            .and_then(|_| stdout.flush())
            .is_err()
        {
            self.exit_code.set(1);
            self.done.set(true);
            if let Some(drop) = self.pending_drop.borrow_mut().take() {
                drop.finish(gdk::DragAction::empty());
            }
            self.main_loop.quit();
            return false;
        }
        true
    }

    fn finish(&self, code: i32, event: &str) {
        if self.done.replace(true) {
            return;
        }
        self.exit_code.set(code);
        self.event(event);
        let main_loop = self.main_loop.clone();
        // Let GTK complete the signal's native protocol cleanup before exiting.
        glib::idle_add_local_once(move || main_loop.quit());
    }

    fn cancel(&self, reason: &str) {
        let pending = self.pending_drop.borrow_mut().take();
        if let Some(drop) = pending {
            drop.finish(gdk::DragAction::empty());
        }
        self.finish(2, &format!("CANCELLED {reason}"));
    }
}

fn clean_message(message: impl std::fmt::Display) -> String {
    message
        .to_string()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

async fn read_paths(drop: &gdk::Drop) -> anyhow::Result<Vec<PathBuf>> {
    if drop.formats().contains_type(gdk::FileList::static_type()) {
        let value = drop
            .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
            .await?;
        let list = value.get::<gdk::FileList>()?;
        let files = list.files();
        if files.len() > MAX_FILES {
            anyhow::bail!("too many dropped files");
        }
        let mut size = 0;
        return files
            .iter()
            .map(|file| {
                let uri = file.uri();
                size += uri.len();
                if size > MAX_URI_BYTES {
                    anyhow::bail!("native URI payload is too large");
                }
                if uri.bytes().any(|byte| byte.is_ascii_control()) {
                    anyhow::bail!("URI contains unescaped control characters");
                }
                parse_file_uri(uri.as_str())
            })
            .collect();
    }
    let (stream, mime) = drop
        .read_future(&["text/uri-list"], glib::Priority::DEFAULT)
        .await?;
    if mime.as_str() != "text/uri-list" {
        anyhow::bail!("drop does not contain a file URI list");
    }
    let result: anyhow::Result<Vec<PathBuf>> = async {
        let mut bytes = Vec::new();
        loop {
            let chunk = stream
                .read_bytes_future(8192, glib::Priority::DEFAULT)
                .await?;
            if chunk.is_empty() {
                break;
            }
            if bytes.len() + chunk.len() > MAX_URI_BYTES {
                anyhow::bail!("native URI payload is too large");
            }
            bytes.extend_from_slice(chunk.as_ref());
        }
        if bytes
            .iter()
            .any(|byte| byte.is_ascii_control() && !matches!(*byte, b'\r' | b'\n'))
        {
            anyhow::bail!("URI list contains unescaped control characters");
        }
        let clipboard = clipboard_decode(&bytes, "text/uri-list")?;
        if clipboard.paths.len() > MAX_FILES {
            anyhow::bail!("too many dropped files");
        }
        Ok(clipboard.paths)
    }
    .await;
    let closed = stream.close_future(glib::Priority::DEFAULT).await;
    let paths = result?;
    closed?;
    Ok(paths)
}

fn send_controller(pad: &gtk::Box, paths: Vec<PathBuf>, session: &Rc<Session>) -> gtk::DragSource {
    let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
    let list = gdk::FileList::from_array(&files);
    let clipboard = Clipboard {
        paths,
        mode: ClipboardMode::Copy,
    };
    let uri_bytes = glib::Bytes::from_owned(clipboard_encode(&clipboard, "text/uri-list"));
    let gnome_bytes =
        glib::Bytes::from_owned(clipboard_encode(&clipboard, "x-special/gnome-copied-files"));
    let provider = gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_value(&list.to_value()),
        gdk::ContentProvider::for_bytes("text/uri-list", &uri_bytes),
        gdk::ContentProvider::for_bytes("x-special/gnome-copied-files", &gnome_bytes),
    ]);
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::COPY);
    source.set_button(1);
    source.set_content(Some(&provider));
    {
        let session = session.clone();
        source.connect_drag_begin(move |_, drag| {
            session.event("DRAGGING");
            let session = session.clone();
            drag.connect_dnd_finished(move |drag| {
                if drag.selected_action() == gdk::DragAction::COPY {
                    session.finish(0, "COMPLETED copy");
                } else {
                    session.cancel("unacknowledged-action");
                }
            });
        });
    }
    {
        let session = session.clone();
        source.connect_drag_cancel(move |_, _, reason| {
            session.cancel(&clean_message(format!("{reason:?}")));
            false
        });
    }
    {
        let session = session.clone();
        source.connect_drag_end(move |_, _, _| {
            let session = session.clone();
            glib::idle_add_local_once(move || {
                if !session.done.get() {
                    session.cancel("destination-did-not-acknowledge");
                }
            });
        });
    }
    pad.add_controller(source.clone());
    source
}

fn receive_controller(pad: &gtk::Box, status: &gtk::Label, session: &Rc<Session>) {
    let formats = gdk::ContentFormats::builder()
        .add_type(gdk::FileList::static_type())
        .add_mime_type("text/uri-list")
        .build();
    let target = gtk::DropTargetAsync::new(Some(formats), gdk::DragAction::COPY);
    {
        let session = session.clone();
        target.connect_accept(move |_, drop| {
            !session.done.get()
                && session.pending_drop.borrow().is_none()
                && drop.actions().contains(gdk::DragAction::COPY)
        });
    }
    target.connect_drag_enter(|_, _, _, _| gdk::DragAction::COPY);
    target.connect_drag_motion(|_, _, _, _| gdk::DragAction::COPY);
    {
        let session = session.clone();
        let status = status.clone();
        target.connect_drop(move |_, drop, _, _| {
            if session.done.get() || session.pending_drop.borrow().is_some() {
                return false;
            }
            *session.pending_drop.borrow_mut() = Some(drop.clone());
            status.set_label("Reading dropped files...");
            let drop = drop.clone();
            let session = session.clone();
            let status = status.clone();
            glib::MainContext::default().spawn_local(async move {
                let paths = read_paths(&drop).await;
                if session.done.get() {
                    return;
                }
                match paths {
                    Ok(paths) if !paths.is_empty() => {
                        if !session.event(&format!("DROP_BEGIN {}", paths.len())) {
                            return;
                        }
                        for path in &paths {
                            if !session.event(&format!("DROP {}", path_uri(path))) {
                                return;
                            }
                        }
                        session.drop_ready.set(true);
                        if !session.event("DROP_END") {
                            return;
                        }
                        status.set_label(
                            "Files received. Waiting for the file manager to finish copying.",
                        );
                    }
                    result => {
                        let message = match result {
                            Err(error) => clean_message(error),
                            _ => "drop contains no files".into(),
                        };
                        if let Some(drop) = session.pending_drop.borrow_mut().take() {
                            drop.finish(gdk::DragAction::empty());
                        }
                        session.finish(1, &format!("ERROR {message}"));
                    }
                }
            });
            true
        });
    }
    pad.add_controller(target);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::Read::take(io::stdin().lock(), 8).read_line(&mut line);
        let accepted = matches!(result, Ok(size) if size > 0)
            && line.trim_end_matches(['\r', '\n']) == "ACCEPT";
        let _ = tx.send(accepted);
    });
    let session = session.clone();
    glib::timeout_add_local(Duration::from_millis(25), move || {
        if session.done.get() {
            return glib::ControlFlow::Break;
        }
        match rx.try_recv() {
            Ok(true) => {
                if !session.drop_ready.get() {
                    session.cancel("acknowledgment-before-files-ready");
                } else {
                    let pending = session.pending_drop.borrow_mut().take();
                    if let Some(drop) = pending {
                        drop.finish(gdk::DragAction::COPY);
                        session.finish(0, "COMPLETED receive");
                    } else {
                        session.finish(1, "ERROR acknowledgment-without-drop");
                    }
                }
                glib::ControlFlow::Break
            }
            Ok(false) | Err(mpsc::TryRecvError::Disconnected) => {
                session.cancel("rejected");
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        }
    });
}

fn run() -> anyhow::Result<i32> {
    let mut args = std::env::args_os().skip(1).peekable();
    let Some(mode) = args.next() else {
        anyhow::bail!("usage: optionfiles-dnd send -- PATH... | receive")
    };
    if mode == "--help" || mode == "-h" {
        eprintln!(
            "Usage: optionfiles-dnd send -- PATH... | receive\n\nSend: drag the file bundle with the left mouse button.\nReceive: drop local files, then write ACCEPT or REJECT to stdin.\nStdout: READY, DRAGGING, DROP_BEGIN count, DROP URI, DROP_END,\nCOMPLETED copy|receive, CANCELLED reason, or ERROR reason.\nExit: 0 acknowledged completion, 2 cancellation, 1 error.\nEscape or closing the window cancels. Sources are never deleted."
        );
        return Ok(0);
    }
    let send = mode == "send";
    if !send && mode != "receive" {
        anyhow::bail!("unknown native drag mode")
    }
    if args.peek().is_some_and(|arg| arg == "--") {
        args.next();
    }
    let paths: Vec<PathBuf> = args
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                Ok(path)
            } else {
                std::env::current_dir().map(|cwd| cwd.join(path))
            }
        })
        .collect::<io::Result<_>>()?;
    if send && paths.is_empty() {
        anyhow::bail!("nothing selected to drag")
    }
    if !send && !paths.is_empty() {
        anyhow::bail!("receive does not take paths")
    }
    for path in &paths {
        path.symlink_metadata()?;
    }
    gtk::init()?;
    let main_loop = glib::MainLoop::new(None, false);
    let session = Rc::new(Session {
        main_loop: main_loop.clone(),
        done: Cell::new(false),
        exit_code: Cell::new(2),
        pending_drop: RefCell::new(None),
        drop_ready: Cell::new(false),
    });
    let window = gtk::Window::builder()
        .title(if send {
            "optionFiles · Send files"
        } else {
            "optionFiles · Receive files"
        })
        .default_width(420)
        .default_height(230)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    body.set_margin_start(16);
    body.set_margin_end(16);
    let pad = gtk::Box::new(gtk::Orientation::Vertical, 8);
    pad.set_vexpand(true);
    pad.set_focusable(true);
    pad.add_css_class("view");
    let heading = gtk::Label::new(Some(if send {
        "Drag this bundle to another app"
    } else {
        "Drop files here"
    }));
    heading.add_css_class("heading");
    heading.set_wrap(true);
    heading.set_margin_top(12);
    pad.append(&heading);
    let status = gtk::Label::new(Some(if send {
        "Hold the left mouse button and drag to the destination. Copy only; originals stay here."
    } else {
        "Drop local files from another app. The file manager chooses where to copy them."
    }));
    status.set_wrap(true);
    status.set_margin_start(12);
    status.set_margin_end(12);
    pad.append(&status);
    if send {
        let names = paths
            .iter()
            .take(5)
            .map(|path| {
                clean_message(
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let list = gtk::Label::new(Some(&format!(
            "{} files\n{names}{}",
            paths.len(),
            if paths.len() > 5 { "\n..." } else { "" }
        )));
        list.set_wrap(true);
        list.set_ellipsize(gtk::pango::EllipsizeMode::End);
        pad.append(&list);
    }
    body.append(&pad);
    let cancel = gtk::Button::with_label("Cancel (Esc)");
    body.append(&cancel);
    window.set_child(Some(&body));
    let source = if send {
        Some(send_controller(&pad, paths, &session))
    } else {
        receive_controller(&pad, &status, &session);
        None
    };
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    let keys = gtk::EventControllerKey::new();
    {
        let window = window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                window.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    window.add_controller(keys);
    {
        let session = session.clone();
        window.connect_close_request(move |_| {
            if let Some(source) = &source {
                if source.drag().is_some() {
                    source.drag_cancel();
                }
            }
            session.cancel("window-closed");
            glib::Propagation::Proceed
        });
    }
    {
        let session = session.clone();
        window.connect_map(move |_| {
            session.event("READY");
        });
    }
    window.present();
    pad.grab_focus();
    if !session.done.get() {
        main_loop.run();
    }
    Ok(session.exit_code.get())
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(io::stdout(), "ERROR {}", clean_message(error));
            1
        }
    };
    std::process::exit(code);
}
