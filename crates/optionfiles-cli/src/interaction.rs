use std::collections::BTreeSet;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use optionfiles_core::{
    App, Clipboard, ClipboardMode, ConflictAction, ConflictResolution, TransferConflict,
    TransferProgress, TransferReport, TransferRequest, clipboard_export, clipboard_import,
    parse_file_uri, run_transfer, trash_path,
};

use crate::dnd::{Dnd, DndEvent};
use crate::input::{Input, InputEvent};
use crate::ui::{Prompt, PromptKind, TerminalUi};

pub struct Pane {
    pub app: Option<App>,
    pub location: PathBuf,
    pub loading: bool,
    pub error: Option<String>,
    receiver: Option<mpsc::Receiver<(App, Result<()>)>>,
    initial: Option<mpsc::Receiver<Result<App>>>,
    range: Option<RangeSelection>,
    refresh_pending: bool,
}

struct RangeSelection {
    anchor: usize,
    baseline: BTreeSet<PathBuf>,
}

impl Pane {
    fn new(path: PathBuf, all: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let location = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(App::new(&path, all));
        });
        Self {
            app: None,
            location,
            loading: true,
            error: None,
            receiver: None,
            initial: Some(rx),
            range: None,
            refresh_pending: false,
        }
    }

    fn refresh(&mut self, all: bool) {
        if self.loading {
            self.refresh_pending = true;
        } else if self.app.is_some() {
            self.job(App::refresh);
        } else {
            *self = Self::new(self.location.clone(), all);
        }
    }

    fn focus(&mut self, index: usize, extend: bool) {
        let Some(app) = &mut self.app else {
            return;
        };
        let index = index.min(app.entries.len().saturating_sub(1));
        if extend {
            let range = self.range.get_or_insert_with(|| RangeSelection {
                anchor: app.selected,
                baseline: app.marked.clone(),
            });
            app.marked.clone_from(&range.baseline);
            let start = range.anchor.min(index);
            let end = range.anchor.max(index);
            app.marked.extend(
                app.entries
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i >= start && *i <= end)
                    .map(|(_, entry)| entry.path.clone()),
            );
        } else {
            self.range = None;
        }
        app.selected = index;
    }

    fn job(&mut self, operation: impl FnOnce(&mut App) -> Result<()> + Send + 'static) {
        let Some(mut app) = self.app.take() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.loading = true;
        self.error = None;
        self.range = None;
        self.receiver = Some(rx);
        std::thread::spawn(move || {
            let result = operation(&mut app);
            let _ = tx.send((app, result));
        });
    }

    fn poll(&mut self) -> bool {
        if let Some(rx) = &self.initial {
            match rx.try_recv() {
                Ok(Ok(app)) => {
                    self.location = app.cwd.clone();
                    self.app = Some(app);
                    self.initial = None;
                    self.loading = false;
                    return true;
                }
                Ok(Err(error)) => {
                    self.error = Some(error.to_string());
                    self.initial = None;
                    self.loading = false;
                    return true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("directory worker stopped".into());
                    self.initial = None;
                    self.loading = false;
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok((mut app, result)) => {
                    self.location = app.cwd.clone();
                    self.error = result.as_ref().err().map(ToString::to_string);
                    if let Err(error) = result {
                        app.status = error.to_string();
                    }
                    self.app = Some(app);
                    self.receiver = None;
                    self.loading = false;
                    return true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("directory worker stopped".into());
                    self.receiver = None;
                    self.loading = false;
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        false
    }
}

pub struct Drag {
    pub sources: Vec<PathBuf>,
    pub origin: (u16, u16),
    pub target: Option<(usize, PathBuf)>,
    pub moved: bool,
}

pub struct Transfer {
    pub progress: Option<TransferProgress>,
    pub conflict: Option<TransferConflict>,
    pub apply_all: bool,
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<TransferEvent>,
    resolution: mpsc::Sender<ConflictResolution>,
    mode: ClipboardMode,
}

enum TransferEvent {
    Progress(TransferProgress),
    Conflict(TransferConflict),
    Finished(TransferReport),
}
enum ClipboardEvent {
    Imported(Option<Clipboard>),
    Exported(Result<Option<&'static str>>),
}

#[derive(Debug, PartialEq, Eq)]
enum NativeEvent {
    Ready,
    Dragging,
    Dropped(Vec<PathBuf>),
    Completed,
    Failed(String),
    Diagnostics(String),
    Closed,
}

#[derive(Default)]
struct NativeDropFrames {
    count: Option<usize>,
    paths: Vec<PathBuf>,
    bytes: usize,
    dropped: bool,
}

impl NativeDropFrames {
    fn line(&mut self, line: &str, receive: bool) -> Result<Option<NativeEvent>> {
        if let Some(count) = line.strip_prefix("DROP_BEGIN ") {
            anyhow::ensure!(
                receive && self.count.is_none() && !self.dropped,
                "unexpected drop"
            );
            let count: usize = count.parse()?;
            anyhow::ensure!((1..=65536).contains(&count), "invalid native drop count");
            self.count = Some(count);
            return Ok(None);
        }
        if let Some(uri) = line.strip_prefix("DROP ") {
            let count = self
                .count
                .ok_or_else(|| anyhow::anyhow!("drop without header"))?;
            anyhow::ensure!(
                !uri.bytes().any(|byte| byte.is_ascii_control()),
                "native URI contains unescaped control characters"
            );
            self.bytes += uri.len();
            anyhow::ensure!(
                self.paths.len() < count && self.bytes <= 16 * 1024 * 1024,
                "native drop exceeds declared limits"
            );
            self.paths.push(parse_file_uri(uri)?);
            return Ok(None);
        }
        if line == "DROP_END" {
            anyhow::ensure!(
                self.count.take() == Some(self.paths.len()) && !self.paths.is_empty(),
                "incomplete native drop"
            );
            self.dropped = true;
            return Ok(Some(NativeEvent::Dropped(std::mem::take(&mut self.paths))));
        }
        anyhow::ensure!(self.count.is_none(), "interrupted native drop");
        Ok(Some(match line {
            "READY" => NativeEvent::Ready,
            "DRAGGING" if !receive => NativeEvent::Dragging,
            "COMPLETED receive" if receive && self.dropped => NativeEvent::Completed,
            "COMPLETED copy" if !receive => NativeEvent::Completed,
            _ if line.starts_with("CANCELLED ") || line.starts_with("ERROR ") => {
                NativeEvent::Failed(line.to_owned())
            }
            _ => anyhow::bail!("invalid native helper response"),
        }))
    }
}

fn native_copy_succeeded(report: &TransferReport, expected: usize) -> bool {
    expected != 0
        && report.completed.len() == expected
        && report.errors.is_empty()
        && report.skipped.is_empty()
        && !report.cancelled
}

struct NativeHelper {
    child: Child,
    stdin: Option<ChildStdin>,
    events: mpsc::Receiver<NativeEvent>,
    readers: [Option<std::thread::JoinHandle<()>>; 2],
    destination: Option<PathBuf>,
    pending: usize,
    acknowledged: Option<bool>,
    completed: bool,
    closed: bool,
    diagnostics: Option<String>,
    failure: Option<String>,
}

impl NativeHelper {
    fn spawn(paths: &[PathBuf], destination: Option<PathBuf>) -> Result<Self> {
        let receive = destination.is_some();
        let helper = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("optionfiles-dnd")))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("optionfiles-dnd"));
        let mut command = Command::new(helper);
        if receive {
            command.arg("receive");
        } else {
            command.args(["send", "--"]).args(paths);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped native helper stdout");
        let stderr = child.stderr.take().expect("piped native helper stderr");
        let (tx, events) = mpsc::channel();
        let diagnostics_tx = tx.clone();
        let diagnostics = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let message = match stderr.take(65536).read_to_end(&mut bytes) {
                Ok(_) => String::from_utf8_lossy(&bytes)
                    .chars()
                    .map(|ch| if ch.is_control() { ' ' } else { ch })
                    .collect(),
                Err(error) => format!("cannot read helper diagnostics: {error}"),
            };
            let _ = diagnostics_tx.send(NativeEvent::Diagnostics(message));
        });
        let reader = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut frames = NativeDropFrames::default();
            let mut line = String::new();
            loop {
                line.clear();
                let result = Read::by_ref(&mut reader)
                    .take(16 * 1024 * 1024 + 1)
                    .read_line(&mut line);
                let event = match result {
                    Ok(0) => break,
                    Ok(_) if line.len() <= 16 * 1024 * 1024 && line.ends_with('\n') => {
                        frames.line(line.trim_end_matches(['\r', '\n']), receive)
                    }
                    Ok(_) => Err(anyhow::anyhow!("native helper frame exceeds limits")),
                    Err(error) => Err(error.into()),
                };
                match event {
                    Ok(Some(event)) => {
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = tx.send(NativeEvent::Failed(error.to_string()));
                        break;
                    }
                }
            }
            let _ = tx.send(NativeEvent::Closed);
        });
        Ok(Self {
            child,
            stdin,
            events,
            readers: [Some(reader), Some(diagnostics)],
            destination,
            pending: 0,
            acknowledged: None,
            completed: false,
            closed: false,
            diagnostics: None,
            failure: None,
        })
    }

    fn acknowledge(&mut self, accepted: bool) -> std::io::Result<()> {
        if self.acknowledged.is_some() {
            return Ok(());
        }
        self.acknowledged = Some(accepted);
        if let Some(mut stdin) = self.stdin.take() {
            stdin.write_all(if accepted { b"ACCEPT\n" } else { b"REJECT\n" })?;
            stdin.flush()?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "native helper control pipe closed",
            ));
        }
        Ok(())
    }
}

impl Drop for NativeHelper {
    fn drop(&mut self) {
        let _ = self.acknowledge(false);
        for _ in 0..10 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in &mut self.readers {
            if let Some(reader) = reader.take() {
                let _ = reader.join();
            }
        }
    }
}

pub struct Session {
    pub panes: Vec<Pane>,
    pub active: usize,
    pub dual: bool,
    pub preview: bool,
    pub help: bool,
    pub help_offset: usize,
    pub prompt: Option<Prompt>,
    pub clipboard: Option<Clipboard>,
    pub transfer: Option<Transfer>,
    pub drag: Option<Drag>,
    pub status: String,
    pub errors: Vec<String>,
    pub error_offset: usize,
    pub show_errors: bool,
    pending_drop: Option<(Vec<PathBuf>, PathBuf)>,
    clipboard_tx: mpsc::Sender<ClipboardEvent>,
    clipboard_rx: mpsc::Receiver<ClipboardEvent>,
    paste_destination: Option<PathBuf>,
    all: bool,
    dnd: Dnd,
    native_destination: Option<PathBuf>,
    native_receiving: bool,
    pub native_armed: bool,
    search_restore: Option<(Option<PathBuf>, usize)>,
    native_drop: bool,
    native_helper: Option<NativeHelper>,
    native_bridge_drop: bool,
}

impl Session {
    fn new(path: &Path, all: bool) -> Self {
        let (clipboard_tx, clipboard_rx) = mpsc::channel();
        Self {
            panes: vec![Pane::new(path.to_path_buf(), all)],
            active: 0,
            dual: false,
            preview: true,
            help: false,
            help_offset: 0,
            prompt: None,
            clipboard: None,
            transfer: None,
            drag: None,
            status: String::new(),
            errors: Vec::new(),
            error_offset: 0,
            show_errors: false,
            pending_drop: None,
            clipboard_tx,
            clipboard_rx,
            paste_destination: None,
            all,
            dnd: Dnd::new(),
            native_destination: None,
            native_drop: false,
            native_receiving: false,
            native_armed: false,
            search_restore: None,
            native_helper: None,
            native_bridge_drop: false,
        }
    }

    fn app(&self) -> Option<&App> {
        self.panes[self.active].app.as_ref()
    }
    fn app_mut(&mut self) -> Option<&mut App> {
        self.panes[self.active].app.as_mut()
    }
    fn job(&mut self, operation: impl FnOnce(&mut App) -> Result<()> + Send + 'static) {
        self.status.clear();
        self.panes[self.active].job(operation);
    }

    fn move_focus(&mut self, delta: isize, extend: bool) {
        if let Some(app) = self.app() {
            let index = app.selected.saturating_add_signed(delta);
            self.panes[self.active].focus(index, extend);
        }
    }

    fn poll(&mut self) -> bool {
        let mut changed = self.poll_native_helper();
        for pane in &mut self.panes {
            changed |= pane.poll();
            if !pane.loading && std::mem::take(&mut pane.refresh_pending) {
                pane.refresh(self.all);
                changed = true;
            }
        }
        while let Ok(message) = self.clipboard_rx.try_recv() {
            changed = true;
            match message {
                ClipboardEvent::Imported(clipboard) => {
                    if let (Some(clipboard), Some(destination)) = (
                        clipboard.or_else(|| self.clipboard.clone()),
                        self.paste_destination.take(),
                    ) {
                        self.clipboard = Some(clipboard.clone());
                        self.start_transfer(clipboard.paths, destination, clipboard.mode);
                    } else {
                        self.status = "clipboard has no local files".into();
                    }
                }
                ClipboardEvent::Exported(Ok(Some(helper))) => {
                    self.status = format!("file clipboard exported ({helper})")
                }
                ClipboardEvent::Exported(Ok(None)) => {
                    self.status = "file clipboard stored internally".into()
                }
                ClipboardEvent::Exported(Err(error)) => {
                    self.status = format!("internal clipboard ready; system clipboard: {error}")
                }
            }
        }
        for _ in 0..64 {
            let event = self
                .transfer
                .as_ref()
                .and_then(|transfer| transfer.receiver.try_recv().ok());
            let Some(event) = event else {
                break;
            };
            changed = true;
            match event {
                TransferEvent::Progress(progress) => {
                    if let Some(transfer) = &mut self.transfer {
                        transfer.progress = Some(progress);
                    }
                }
                TransferEvent::Conflict(conflict) => {
                    if let Some(transfer) = &mut self.transfer {
                        transfer.conflict = Some(conflict);
                    }
                }
                TransferEvent::Finished(report) => {
                    let mode = self.transfer.take().map(|t| t.mode);
                    let mut acknowledgement_error = None;
                    if self.native_bridge_drop {
                        self.native_bridge_drop = false;
                        if let Some(helper) = &mut self.native_helper {
                            let accepted = native_copy_succeeded(&report, helper.pending);
                            if let Err(error) = helper.acknowledge(accepted) {
                                helper.failure = Some(error.to_string());
                                acknowledgement_error = Some(error);
                            }
                        }
                    }
                    if self.native_drop {
                        self.native_drop = false;
                        let result = if !report.completed.is_empty()
                            && report.errors.is_empty()
                            && report.skipped.is_empty()
                            && !report.cancelled
                        {
                            self.dnd.accept_drop(&mut std::io::stdout())
                        } else {
                            self.dnd.reject_drop(&mut std::io::stdout())
                        };
                        acknowledgement_error = result.err();
                    }
                    if mode == Some(ClipboardMode::Cut) {
                        if let Some(clipboard) = &mut self.clipboard {
                            clipboard
                                .paths
                                .retain(|path| !report.completed.contains(path));
                            if clipboard.paths.is_empty() {
                                self.clipboard = None;
                            }
                        }
                    }
                    self.status = format!(
                        "{} completed · {} skipped · {} errors{}",
                        report.completed.len(),
                        report.skipped.len(),
                        report.errors.len(),
                        if report.cancelled {
                            " · cancelled"
                        } else {
                            ""
                        }
                    );
                    self.errors = report
                        .errors
                        .into_iter()
                        .map(|e| {
                            format!(
                                "{} -> {}: {}",
                                e.source.display(),
                                e.target.display(),
                                e.message
                            )
                        })
                        .collect();
                    self.error_offset = 0;
                    if !self.errors.is_empty() {
                        self.status.push_str(" · ! details");
                    }
                    if let Some(error) = acknowledgement_error {
                        self.status
                            .push_str(&format!(" · native acknowledgement: {error}"));
                    }
                    for pane in &mut self.panes {
                        pane.refresh(self.all);
                    }
                }
            }
        }
        changed
    }

    fn start_transfer(&mut self, sources: Vec<PathBuf>, destination: PathBuf, mode: ClipboardMode) {
        if self.transfer.is_some() {
            self.status = "transfer running · ctrl+c cancels".into();
            return;
        }
        if sources.is_empty() {
            self.status = "no selected files".into();
            return;
        }
        let request = TransferRequest {
            sources,
            destination,
            mode,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, receiver) = mpsc::sync_channel(32);
        let (resolution, answers) = mpsc::channel();
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let conflict_tx = tx.clone();
            let report = run_transfer(
                &request,
                &worker_cancel,
                |p| {
                    let _ = progress_tx.send(TransferEvent::Progress(TransferProgress {
                        items_done: p.items_done,
                        items_total: p.items_total,
                        bytes_done: p.bytes_done,
                        current: p.current.clone(),
                    }));
                },
                |c| {
                    let _ = conflict_tx.send(TransferEvent::Conflict(TransferConflict {
                        source: c.source.clone(),
                        target: c.target.clone(),
                    }));
                    loop {
                        match answers.recv_timeout(Duration::from_millis(50)) {
                            Ok(answer) => return answer,
                            Err(mpsc::RecvTimeoutError::Timeout)
                                if !worker_cancel.load(Ordering::Acquire) => {}
                            _ => {
                                return ConflictResolution {
                                    action: ConflictAction::Skip,
                                    apply_to_batch: false,
                                };
                            }
                        }
                    }
                },
            );
            let _ = tx.send(TransferEvent::Finished(report));
        });
        self.transfer = Some(Transfer {
            progress: None,
            conflict: None,
            apply_all: false,
            cancel,
            receiver,
            resolution,
            mode,
        });
        self.status = "transfer started · ctrl+c cancels".into();
    }

    fn copy(&mut self, mode: ClipboardMode) {
        let Some(app) = self.app() else {
            return;
        };
        let clipboard = Clipboard {
            paths: app.selected_paths(),
            mode,
        };
        if clipboard.paths.is_empty() {
            return;
        }
        self.status = format!(
            "{} {} selected",
            if mode == ClipboardMode::Copy {
                "copy"
            } else {
                "cut"
            },
            clipboard.paths.len()
        );
        self.clipboard = Some(clipboard.clone());
        let tx = self.clipboard_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ClipboardEvent::Exported(clipboard_export(&clipboard)));
        });
    }

    fn paste(&mut self) {
        if self.transfer.is_some() || self.paste_destination.is_some() {
            self.status = "paste already running".into();
            return;
        }
        let Some(destination) = self.app().map(|app| app.cwd.clone()) else {
            return;
        };
        self.paste_destination = Some(destination);
        let tx = self.clipboard_tx.clone();
        self.status = "reading system file clipboard".into();
        std::thread::spawn(move || {
            let _ = tx.send(ClipboardEvent::Imported(clipboard_import()));
        });
    }

    fn drop_paths(&mut self, sources: Vec<PathBuf>, destination: PathBuf) {
        if self.transfer.is_some() {
            self.status = "transfer running · ctrl+c cancels".into();
            return;
        }
        self.pending_drop = Some((sources, destination));
        self.prompt = Some(Prompt {
            kind: PromptKind::Drop,
            value: String::new(),
            original: String::new(),
        });
    }

    fn prompt(&mut self, kind: PromptKind, value: String) {
        if matches!(kind, PromptKind::Search) {
            self.panes[self.active].range = None;
            self.search_restore = self
                .app()
                .map(|app| (app.current().map(|entry| entry.path.clone()), app.scroll));
        }
        self.prompt = Some(Prompt {
            kind,
            original: value.clone(),
            value,
        });
    }

    fn handle_prompt(&mut self, code: KeyCode) {
        if code == KeyCode::Esc {
            if let Some(prompt) = self.prompt.take() {
                if matches!(prompt.kind, PromptKind::Search) {
                    let restore = self.search_restore.take();
                    if let Some(app) = self.app_mut() {
                        let _ = app.set_filter(prompt.original);
                        if let Some((focused, scroll)) = restore {
                            if let Some(index) = focused.and_then(|path| {
                                app.entries.iter().position(|entry| entry.path == path)
                            }) {
                                app.selected = index;
                            }
                            app.scroll = scroll.min(app.entries.len().saturating_sub(1));
                        }
                    }
                }
            }
            self.pending_drop = None;
            return;
        }
        if code == KeyCode::Enter {
            let prompt = self.prompt.take().unwrap();
            let name = prompt.value.trim().to_string();
            match prompt.kind {
                PromptKind::Folder => self.job(move |app| app.create_dir(&name)),
                PromptKind::File => self.job(move |app| app.create_file(&name)),
                PromptKind::Rename => self.job(move |app| app.rename_current(&name)),
                PromptKind::Delete if name.eq_ignore_ascii_case("y") => self.job(|app| {
                    let mut errors = Vec::new();
                    for path in app.selected_paths() {
                        if let Err(error) = trash_path(&path) {
                            errors.push(format!("{}: {error}", path.display()));
                        }
                    }
                    app.refresh()?;
                    app.status = if errors.is_empty() {
                        "selection moved to trash".into()
                    } else {
                        errors.join("; ")
                    };
                    Ok(())
                }),
                PromptKind::Delete => self.status = "trash cancelled".into(),
                PromptKind::Search => {
                    self.search_restore = None;
                }
                PromptKind::Drop => {
                    if let Some((sources, destination)) = self.pending_drop.take() {
                        match name.as_str() {
                            "c" | "copy" => {
                                self.start_transfer(sources, destination, ClipboardMode::Copy)
                            }
                            "m" | "move" => {
                                self.start_transfer(sources, destination, ClipboardMode::Cut)
                            }
                            _ => {
                                self.status = "drop cancelled: choose c (copy) or m (move)".into();
                            }
                        }
                    }
                }
            }
            return;
        }
        if let Some(prompt) = &mut self.prompt {
            match code {
                KeyCode::Backspace => {
                    prompt.value.pop();
                }
                KeyCode::Char(c) if !c.is_control() => prompt.value.push(c),
                _ => {}
            }
            if matches!(prompt.kind, PromptKind::Search) {
                let query = prompt.value.clone();
                if let Some(app) = self.app_mut() {
                    let _ = app.set_filter(query);
                }
            }
        }
    }

    fn pasted(&mut self, value: &str) {
        if let Some(prompt) = &mut self.prompt {
            prompt
                .value
                .extend(value.chars().filter(|c| !c.is_control()));
            if matches!(prompt.kind, PromptKind::Search) {
                let query = prompt.value.clone();
                if let Some(app) = self.app_mut() {
                    let _ = app.set_filter(query);
                }
            }
            return;
        }
        if self.help
            || self.show_errors
            || self.transfer.as_ref().is_some_and(|t| t.conflict.is_some())
        {
            return;
        }
        match parse_pasted_paths(value) {
            Ok(paths) => {
                if let Some(destination) = self.app().map(|app| app.cwd.clone()) {
                    self.drop_paths(paths, destination);
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers, terminal: &mut TerminalUi) -> bool {
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(transfer) = &self.transfer {
                transfer.cancel.store(true, Ordering::Release);
                if self.native_bridge_drop {
                    if let Some(helper) = &mut self.native_helper {
                        let _ = helper.acknowledge(false);
                    }
                }
                self.status = "cancelling transfer".into();
                return false;
            }
            if self.native_helper.take().is_some() {
                self.status = "native helper cancelled".into();
                return false;
            }
            return true;
        }
        if modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        if modifiers.contains(KeyModifiers::CONTROL)
            && !matches!(code, KeyCode::Char('a' | 'd' | 'f' | 'r' | 'v'))
        {
            return false;
        }
        if self.prompt.is_some() {
            if !modifiers.contains(KeyModifiers::CONTROL) {
                self.handle_prompt(code);
            }
            return false;
        }
        if self.help {
            match code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.help = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_offset = (self.help_offset + 1).min(crate::ui::help_limit())
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_offset = self.help_offset.saturating_sub(1)
                }
                _ => {}
            }
            return false;
        }
        if self.show_errors {
            match code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('!') => self.show_errors = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.error_offset =
                        (self.error_offset + 1).min(crate::ui::overlay_limit(&self.errors))
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.error_offset = self.error_offset.saturating_sub(1)
                }
                _ => {}
            }
            return false;
        }
        if let Some(transfer) = &mut self.transfer {
            if transfer.conflict.is_some() {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    return false;
                }
                let action = match code {
                    KeyCode::Char('o') => Some(ConflictAction::Overwrite),
                    KeyCode::Char('s') => Some(ConflictAction::Skip),
                    KeyCode::Char('k') => Some(ConflictAction::KeepBoth),
                    KeyCode::Char('a') => {
                        transfer.apply_all = !transfer.apply_all;
                        None
                    }
                    KeyCode::Esc => {
                        transfer.cancel.store(true, Ordering::Release);
                        None
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    let _ = transfer.resolution.send(ConflictResolution {
                        action,
                        apply_to_batch: transfer.apply_all,
                    });
                    transfer.conflict = None;
                }
                return false;
            }
        }
        let independent = matches!(
            code,
            KeyCode::Char('q' | '?' | '!' | 'w' | ' ' | 'p' | '[' | ']' | 'D')
                | KeyCode::Esc
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::F(5)
        ) || (code == KeyCode::Char('r')
            && modifiers.contains(KeyModifiers::CONTROL));
        if self.app().is_none() && !independent {
            self.status = if self.panes[self.active].loading {
                "directory loading · navigation waits for its result"
            } else {
                "directory unavailable · ctrl+r retries"
            }
            .into();
            return false;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('!') => self.show_errors = !self.errors.is_empty(),
            KeyCode::Tab | KeyCode::BackTab if self.dual => {
                self.drag = None;
                self.active = 1 - self.active;
            }
            KeyCode::Char('w') => {
                self.dual = !self.dual;
                if self.dual && self.panes.len() == 1 {
                    self.panes
                        .push(Pane::new(self.panes[0].location.clone(), self.all));
                }
                if !self.dual {
                    self.active = 0;
                }
            }
            KeyCode::Char(' ') => self.preview = !self.preview,
            KeyCode::Char('p') => terminal.toggle_preview_text(),
            KeyCode::Char('[') => terminal.scroll_preview(-5),
            KeyCode::Char(']') => terminal.scroll_preview(5),
            KeyCode::Char('c') => self.copy(ClipboardMode::Copy),
            KeyCode::Char('x') => self.copy(ClipboardMode::Cut),
            KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_native_helper(true)
            }
            KeyCode::Char('v') => self.paste(),
            KeyCode::F(5) if self.dual => self.to_other(ClipboardMode::Copy),
            KeyCode::F(6) if self.dual => self.to_other(ClipboardMode::Cut),
            KeyCode::Char('D') => self.native_drag(),
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_native_helper(false)
            }
            KeyCode::Char('f') if modifiers.contains(KeyModifiers::CONTROL) => {
                let value = self.app().map(|app| app.filter.clone()).unwrap_or_default();
                self.prompt(PromptKind::Search, value);
            }
            KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.status.clear();
                self.panes[self.active].refresh(self.all);
            }
            KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.panes[self.active].range = None;
                if let Some(app) = self.app_mut() {
                    app.select_all();
                }
            }
            KeyCode::Char('u') => {
                self.panes[self.active].range = None;
                if let Some(app) = self.app_mut() {
                    app.clear_selection();
                }
            }
            KeyCode::Char('t') | KeyCode::Insert => {
                self.panes[self.active].range = None;
                if let Some(app) = self.app_mut() {
                    app.toggle_selection();
                    app.move_by(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_focus(-1, modifiers.contains(KeyModifiers::SHIFT))
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_focus(1, modifiers.contains(KeyModifiers::SHIFT))
            }
            KeyCode::PageUp => self.move_focus(-10, modifiers.contains(KeyModifiers::SHIFT)),
            KeyCode::PageDown => self.move_focus(10, modifiers.contains(KeyModifiers::SHIFT)),
            KeyCode::Home | KeyCode::Char('g') => {
                self.panes[self.active].focus(0, modifiers.contains(KeyModifiers::SHIFT))
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.panes[self.active].focus(usize::MAX, modifiers.contains(KeyModifiers::SHIFT))
            }
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.job(App::parent),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.job(|app| {
                if let Some(path) = app.enter()? {
                    crate::open_external(&path, app);
                }
                Ok(())
            }),
            KeyCode::Char('o') => {
                self.status.clear();
                if let Some(app) = self.app_mut() {
                    if let Some(path) = app.current().map(|e| e.path.clone()) {
                        crate::open_external(&path, app);
                    }
                }
            }
            KeyCode::Char('e') => {
                if self.transfer.is_some() || self.paste_destination.is_some() {
                    self.status = "finish the transfer before opening a terminal editor".into();
                } else if let Some(path) = self
                    .app()
                    .and_then(|app| app.current())
                    .map(|entry| entry.path.clone())
                {
                    let stop = self.dnd.stop(&mut std::io::stdout());
                    self.native_armed = false;
                    self.native_receiving = false;
                    self.native_destination = None;
                    let result = stop
                        .map_err(anyhow::Error::from)
                        .and_then(|()| terminal.suspend());
                    if result.is_ok() {
                        if let Some(app) = self.app_mut() {
                            crate::edit_with_editor(&path, app);
                        }
                    }
                    let restored = terminal.resume().and_then(|()| {
                        self.dnd
                            .start(&mut std::io::stdout())
                            .map_err(anyhow::Error::from)
                    });
                    if let Err(error) = result.and(restored) {
                        self.status = format!("terminal editor: {error}");
                    } else {
                        self.job(App::refresh);
                    }
                }
            }
            KeyCode::Char('a') | KeyCode::Char('.') => {
                self.panes[self.active].range = None;
                if let Some(app) = self.app_mut() {
                    let _ = app.toggle_hidden();
                }
            }
            KeyCode::Char('s') => {
                self.panes[self.active].range = None;
                if let Some(app) = self.app_mut() {
                    let _ = app.cycle_sort();
                }
            }
            KeyCode::Char('/') => {
                let value = self.app().map(|app| app.filter.clone()).unwrap_or_default();
                self.prompt(PromptKind::Search, value);
            }
            KeyCode::Char('~') => self.job(App::go_home),
            KeyCode::Char('-') => self.job(App::go_previous),
            KeyCode::Char('n') => self.prompt(PromptKind::Folder, String::new()),
            KeyCode::Char('N') => self.prompt(PromptKind::File, String::new()),
            KeyCode::Char('r') | KeyCode::F(2) => {
                if let Some(value) = self
                    .app()
                    .and_then(|app| app.current())
                    .map(|e| e.name.clone())
                {
                    self.prompt(PromptKind::Rename, value);
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => self.prompt(PromptKind::Delete, String::new()),
            KeyCode::F(5) => {
                self.status.clear();
                self.panes[self.active].refresh(self.all);
            }
            _ => {}
        }
        false
    }

    fn to_other(&mut self, mode: ClipboardMode) {
        if let (Some(sources), Some(destination)) = (
            self.app().map(App::selected_paths),
            self.panes
                .get(1 - self.active)
                .and_then(|p| p.app.as_ref())
                .map(|app| app.cwd.clone()),
        ) {
            self.start_transfer(sources, destination, mode);
        } else {
            self.status = "both directories must finish loading before a pane transfer".into();
        }
    }

    fn osc(&mut self, frame: &str, terminal: &TerminalUi) {
        let metadata = frame
            .strip_prefix("72;")
            .and_then(|body| body.split(';').next())
            .unwrap_or("");
        let is_drop = metadata.split(':').any(|part| part == "t=M");
        if is_drop {
            let coordinate = |key: &str| {
                metadata.split(':').find_map(|part| {
                    part.strip_prefix(key)
                        .and_then(|n| n.parse::<i32>().ok())
                        .and_then(|n| u16::try_from(n).ok())
                })
            };
            self.native_destination = coordinate("x=")
                .zip(coordinate("y="))
                .and_then(|(x, y)| terminal.destination(x, y, self));
            self.native_receiving = true;
        }
        let result = self.dnd.handle(frame, &mut std::io::stdout());
        match result {
            Ok(Some(DndEvent::Supported)) => {
                self.status =
                    "native protocol ready · ctrl+v receive pad · ctrl+d send handle".into()
            }
            Ok(Some(DndEvent::OfferGesture { x, y })) => {
                if !self.native_armed || self.prompt.is_some() || self.help || self.show_errors {
                    return;
                }
                if let Some((pane, index)) = terminal.hit(x, y, self) {
                    self.active = pane;
                    self.panes[pane].focus(index, false);
                    if let Some(app) = self.app_mut() {
                        if app
                            .current()
                            .is_some_and(|entry| !app.marked.contains(&entry.path))
                        {
                            app.clear_selection();
                        }
                    }
                    let paths = self.app().map(App::selected_paths).unwrap_or_default();
                    self.drag = None;
                    if let Err(error) = self.dnd.begin_offer(paths, &mut std::io::stdout()) {
                        self.status = format!("native drag: {error}");
                    }
                }
            }
            Ok(Some(DndEvent::Hover { x, y })) => {
                if !self.native_receiving && !self.native_drop {
                    self.native_destination = u16::try_from(x)
                        .ok()
                        .zip(u16::try_from(y).ok())
                        .and_then(|(x, y)| terminal.destination(x, y, self));
                }
            }
            Ok(Some(DndEvent::Dropped(paths))) => {
                self.native_receiving = false;
                if self.transfer.is_some() || self.prompt.is_some() || self.help || self.show_errors
                {
                    let _ = self.dnd.reject_drop(&mut std::io::stdout());
                    self.native_destination = None;
                    self.status = "native drop rejected: finish the active action first".into();
                } else if let Some(destination) = self.native_destination.take() {
                    self.native_drop = true;
                    self.start_transfer(paths, destination, ClipboardMode::Copy);
                } else {
                    let _ = self.dnd.reject_drop(&mut std::io::stdout());
                    self.status = "native drop rejected: drop inside a directory list".into();
                }
            }
            Ok(Some(DndEvent::Started)) => self.status = "native copy drag started".into(),
            Ok(Some(DndEvent::Completed)) => self.status = "native copy drag completed".into(),
            Ok(Some(DndEvent::Cancelled)) => self.status = "native drag cancelled".into(),
            Ok(Some(DndEvent::Error(error))) => {
                self.native_receiving = false;
                self.native_destination = None;
                self.status = format!("native drag: {error}");
            }
            Err(error) => {
                self.native_receiving = false;
                self.native_destination = None;
                self.status = format!("native drag: {error}");
            }
            Ok(None) => {
                if is_drop
                    && (self.native_destination.is_none()
                        || self.transfer.is_some()
                        || self.prompt.is_some()
                        || self.help
                        || self.show_errors)
                {
                    let _ = self.dnd.reject_drop(&mut std::io::stdout());
                    self.native_receiving = false;
                    self.native_destination = None;
                    self.status = "native drop rejected: choose an idle directory list".into();
                }
            }
        }
    }

    fn native_drag(&mut self) {
        if self.dnd.supported() {
            let armed = !self.native_armed;
            match self.dnd.arm_source(armed, &mut std::io::stdout()) {
                Ok(()) => {
                    self.native_armed = self.dnd.source_armed();
                    self.drag = None;
                    self.status = if armed {
                        "direct source armed · Kitty Wayland finish limited · ctrl+d uses GTK"
                    } else {
                        "internal drag restored"
                    }
                    .into();
                }
                Err(error) => self.status = format!("native source: {error}"),
            }
            return;
        }
        self.open_native_helper(false);
    }

    fn open_native_helper(&mut self, receive: bool) {
        if self.native_helper.is_some() || self.transfer.is_some() || self.native_receiving {
            self.status = "finish the active transfer or native helper first".into();
            return;
        }
        let Some(app) = self.app() else {
            return;
        };
        let (paths, destination) = if receive {
            (Vec::new(), Some(app.cwd.clone()))
        } else {
            let paths = app.selected_paths();
            if paths.is_empty() {
                self.status = "no selected files".into();
                return;
            }
            (paths, None)
        };
        match NativeHelper::spawn(&paths, destination) {
            Ok(helper) => {
                self.native_helper = Some(helper);
                self.status = if receive {
                    "opening native receive pad · destination fixed to current directory"
                } else {
                    "opening native send handle · drag it to the destination"
                }
                .into();
            }
            Err(error) => self.status = format!("cannot open native helper: {error}"),
        }
    }

    fn poll_native_helper(&mut self) -> bool {
        let Some(mut helper) = self.native_helper.take() else {
            return false;
        };
        let mut changed = false;
        while let Ok(event) = helper.events.try_recv() {
            changed = true;
            match event {
                NativeEvent::Ready => {
                    self.status = if let Some(destination) = &helper.destination {
                        format!(
                            "drop onto native pad to copy into {}",
                            destination.display()
                        )
                    } else {
                        "native send handle ready · drag with the left mouse button".into()
                    };
                }
                NativeEvent::Dragging => self.status = "native copy drag started".into(),
                NativeEvent::Dropped(paths) => {
                    if helper.pending != 0
                        || self.transfer.is_some()
                        || helper.acknowledged.is_some()
                    {
                        let _ = helper.acknowledge(false);
                        helper.failure = Some("native drop rejected: transfer is busy".into());
                    } else if let Some(destination) = helper.destination.clone() {
                        helper.pending = paths.len();
                        self.native_bridge_drop = true;
                        self.start_transfer(paths, destination, ClipboardMode::Copy);
                    }
                }
                NativeEvent::Completed => {
                    if helper.destination.is_none() || helper.acknowledged == Some(true) {
                        helper.completed = true;
                    } else {
                        helper.failure =
                            Some("native helper completed without copy acceptance".into());
                    }
                }
                NativeEvent::Failed(error) => {
                    let _ = helper.acknowledge(false);
                    helper.failure = Some(error);
                }
                NativeEvent::Diagnostics(message) => helper.diagnostics = Some(message),
                NativeEvent::Closed => helper.closed = true,
            }
        }
        let exited = match helper.child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                helper.failure = Some(error.to_string());
                helper.closed = true;
                let _ = helper.child.kill();
                helper.child.wait().ok()
            }
        };
        if let Some(status) = exited.filter(|_| helper.closed && helper.diagnostics.is_some()) {
            if self.native_bridge_drop && helper.acknowledged != Some(true) {
                if let Some(transfer) = &self.transfer {
                    transfer.cancel.store(true, Ordering::Release);
                }
            }
            self.status = if status.success() && helper.completed && helper.failure.is_none() {
                if helper.destination.is_some() {
                    "native receive completed · files copied".into()
                } else {
                    "native copy drag completed".into()
                }
            } else {
                match &helper.failure {
                    Some(error) => format!("native helper: {error}"),
                    None if helper.diagnostics.as_ref().is_some_and(|s| !s.is_empty()) => {
                        format!("native helper: {}", helper.diagnostics.as_deref().unwrap())
                    }
                    None => format!("native helper: {status} without acknowledged completion"),
                }
            };
            return true;
        }
        if helper.failure.is_some() {
            let _ = helper.child.kill();
        }
        self.native_helper = Some(helper);
        changed
    }

    fn mouse(&mut self, mouse: crossterm::event::MouseEvent, terminal: &mut TerminalUi) {
        if self.prompt.is_some()
            || self.help
            || self.show_errors
            || self.transfer.as_ref().is_some_and(|t| t.conflict.is_some())
        {
            return;
        }
        if terminal.preview_contains(mouse.column, mouse.row) {
            match mouse.kind {
                MouseEventKind::ScrollUp => terminal.scroll_preview(-3),
                MouseEventKind::ScrollDown => terminal.scroll_preview(3),
                _ => {}
            }
            return;
        }
        let hit = terminal.hit(mouse.column, mouse.row, self);
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some((pane, _)) = hit {
                    self.active = pane;
                    self.move_focus(
                        if mouse.kind == MouseEventKind::ScrollUp {
                            -3
                        } else {
                            3
                        },
                        false,
                    );
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag = None;
                if let Some((pane, index)) = hit {
                    self.active = pane;
                    self.panes[pane].focus(index, mouse.modifiers.contains(KeyModifiers::SHIFT));
                    if let Some(app) = self.app_mut() {
                        if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                            app.toggle_selection();
                        } else if !mouse.modifiers.contains(KeyModifiers::SHIFT)
                            && app
                                .current()
                                .is_some_and(|entry| !app.marked.contains(&entry.path))
                        {
                            app.clear_selection();
                        }
                        let sources = app.selected_paths();
                        self.drag = Some(Drag {
                            sources,
                            origin: (mouse.column, mouse.row),
                            target: None,
                            moved: false,
                        });
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let target = terminal.destination_pane(mouse.column, mouse.row, self);
                if let Some(drag) = &mut self.drag {
                    drag.moved |= drag.origin != (mouse.column, mouse.row);
                    drag.target = target;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let destination = terminal.destination(mouse.column, mouse.row, self);
                if let Some(drag) = self.drag.take() {
                    if drag.moved {
                        if let Some(destination) = destination {
                            self.drop_paths(drag.sources, destination);
                        } else {
                            self.status = "drop cancelled: release inside a directory list".into();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(transfer) = &self.transfer {
            transfer.cancel.store(true, Ordering::Release);
        }
        let _ = self.dnd.stop(&mut std::io::stdout());
    }
}

pub fn run(path: &Path, all: bool) -> Result<()> {
    let mut terminal = TerminalUi::enter()?;
    let mut session = Session::new(path, all);
    let mut input = Input::new();
    session.dnd.start(&mut std::io::stdout())?;
    let mut size = crossterm::terminal::size()?;
    let mut dirty = true;
    loop {
        dirty |= session.poll();
        dirty |= terminal.poll_preview();
        let current_size = crossterm::terminal::size()?;
        if current_size != size {
            size = current_size;
            session.drag = None;
            dirty = true;
        }
        if dirty {
            terminal.draw(&mut session)?;
            dirty = false;
        }
        let Some(event) = input.poll(Duration::from_millis(40))? else {
            continue;
        };
        dirty = true;
        match event {
            InputEvent::Terminal(Event::Key(key))
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
            {
                if session.key(key.code, key.modifiers, &mut terminal) {
                    break;
                }
            }
            InputEvent::Terminal(Event::Paste(value)) => session.pasted(&value),
            InputEvent::Terminal(Event::Mouse(mouse)) => session.mouse(mouse, &mut terminal),
            InputEvent::Osc(frame) => session.osc(&frame, &terminal),
            InputEvent::PrimaryAttributes(attributes) => {
                if !attributes.is_empty() && session.dnd.primary_attributes() {
                    session.status =
                        "native bridge ready · ctrl+v receive pad · ctrl+d send handle".into();
                }
            }
            InputEvent::Error(error) => session.status = error,
            _ => {}
        }
    }
    Ok(())
}

pub fn parse_pasted_paths(value: &str) -> std::result::Result<Vec<PathBuf>, String> {
    if value.len() > 1024 * 1024 {
        return Err("pasted paths exceed 1 MiB".into());
    }
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("pasted paths contain control characters".into());
    }
    let value = value.trim();
    if value.is_empty() {
        return Err("no pasted paths".into());
    }
    let mut tokens = Vec::new();
    if value.lines().any(|line| line.trim().starts_with("file://")) {
        for line in value
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            tokens.push(line.to_string());
        }
    } else {
        let mut token = String::new();
        let mut quote = None;
        let mut escaped = false;
        for c in value.chars() {
            if escaped {
                token.push(c);
                escaped = false;
            } else if c == '\\' && quote != Some('\'') {
                escaped = true;
            } else if let Some(q) = quote {
                if c == q {
                    quote = None;
                } else {
                    token.push(c);
                }
            } else if matches!(c, '\'' | '"') {
                quote = Some(c);
            } else if c.is_whitespace() {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            } else {
                token.push(c);
            }
        }
        if quote.is_some() || escaped {
            return Err("unfinished quote or escape in pasted paths".into());
        }
        if !token.is_empty() {
            tokens.push(token);
        }
    }
    let mut paths = Vec::new();
    for token in tokens {
        let path = if let Some(uri) = token.strip_prefix("file://") {
            let local = if uri.starts_with('/') {
                uri.to_string()
            } else if let Some(local) = uri.strip_prefix("localhost/") {
                format!("/{local}")
            } else {
                return Err("only local file:// URLs can be pasted".into());
            };
            let bytes = local.as_bytes();
            let mut decoded = Vec::new();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] == b'%' {
                    let Some(pair) = bytes.get(index + 1..index + 3) else {
                        return Err("invalid percent escape in pasted URL".into());
                    };
                    let hex = std::str::from_utf8(pair).map_err(|_| "invalid URL escape")?;
                    decoded.push(u8::from_str_radix(hex, 16).map_err(|_| "invalid URL escape")?);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            if decoded.iter().any(|b| *b == 0 || *b < 32 || *b == 127) {
                return Err("pasted URL contains control characters".into());
            }
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(decoded))
        } else if token == "~" {
            option_sdk::home_dir()
        } else if let Some(relative) = token.strip_prefix("~/") {
            option_sdk::home_dir().join(relative)
        } else {
            PathBuf::from(token)
        };
        if !path.is_absolute() {
            return Err("paste absolute local paths (quote paths containing spaces)".into());
        }
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        Err("no local paths in paste".into())
    } else {
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_drop_frames_deliver_only_complete_local_uri_bundles() {
        let mut frames = NativeDropFrames::default();
        assert_eq!(
            frames.line("READY", true).unwrap(),
            Some(NativeEvent::Ready)
        );
        assert_eq!(frames.line("DROP_BEGIN 2", true).unwrap(), None);
        assert_eq!(
            frames.line("DROP file:///tmp/two%20words", true).unwrap(),
            None
        );
        assert_eq!(
            frames.line("DROP file:///tmp/caf%C3%A9", true).unwrap(),
            None
        );
        assert_eq!(
            frames.line("DROP_END", true).unwrap(),
            Some(NativeEvent::Dropped(vec![
                PathBuf::from("/tmp/two words"),
                PathBuf::from("/tmp/café"),
            ]))
        );
        assert_eq!(
            frames.line("COMPLETED receive", true).unwrap(),
            Some(NativeEvent::Completed)
        );
        assert!(frames.line("DROP_BEGIN 1", true).is_err());
    }

    #[test]
    fn native_drop_frames_reject_unframed_partial_remote_and_wrong_mode_data() {
        for line in [
            "DROP file:///tmp/a",
            "DROP_END",
            "COMPLETED receive",
            "DROP_BEGIN 0",
            "DROP_BEGIN 65537",
        ] {
            assert!(NativeDropFrames::default().line(line, true).is_err());
        }
        assert!(
            NativeDropFrames::default()
                .line("DROP_BEGIN 1", false)
                .is_err()
        );
        for line in [
            "DROP file://remote/tmp/a",
            "DROP file:///tmp/a%00",
            "DROP file:///tmp/a\tb",
            "DROP_END",
            "COMPLETED receive",
        ] {
            let mut frames = NativeDropFrames::default();
            frames.line("DROP_BEGIN 1", true).unwrap();
            assert!(frames.line(line, true).is_err(), "{line}");
        }
        let mut frames = NativeDropFrames::default();
        frames.line("DROP_BEGIN 2", true).unwrap();
        frames.line("DROP file:///tmp/a", true).unwrap();
        assert!(frames.line("DROP_END", true).is_err());
    }

    #[test]
    fn native_acceptance_requires_every_copy_without_skip_error_or_cancellation() {
        let mut report = TransferReport {
            completed: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")],
            ..TransferReport::default()
        };
        assert!(native_copy_succeeded(&report, 2));
        assert!(!native_copy_succeeded(&report, 0));
        assert!(!native_copy_succeeded(&report, 3));
        report.cancelled = true;
        assert!(!native_copy_succeeded(&report, 2));
        report.cancelled = false;
        report.skipped.push(PathBuf::from("/tmp/c"));
        assert!(!native_copy_succeeded(&report, 2));
        report.skipped.clear();
        report.errors.push(optionfiles_core::TransferError {
            source: PathBuf::from("/tmp/c"),
            target: PathBuf::from("/tmp/d"),
            message: "copy failed".into(),
        });
        assert!(!native_copy_succeeded(&report, 2));
    }
    #[test]
    fn pasted_paths_are_data_not_commands() {
        assert_eq!(
            parse_pasted_paths("'/tmp/a b' /tmp/c\\ d").unwrap(),
            vec![PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c d")]
        );
        assert!(parse_pasted_paths("q\nrm -rf /").is_err());
        assert!(parse_pasted_paths("/tmp/a\x1b[31m").is_err());
        assert!(parse_pasted_paths("file://remote/tmp/a").is_err());
        assert!(parse_pasted_paths("file:///tmp/a%00").is_err());
        assert_eq!(
            parse_pasted_paths("file:///tmp/a%20b\nfile:///tmp/c").unwrap(),
            vec![PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c")]
        );
    }

    #[test]
    fn pasted_paths_reject_malformed_local_urls_and_deduplicate() {
        assert!(parse_pasted_paths("file:///tmp/a%").is_err());
        assert!(parse_pasted_paths("file:///tmp/a%ZZ").is_err());
        assert!(parse_pasted_paths("file:///tmp/a%1b[31m").is_err());
        assert!(parse_pasted_paths("'/tmp/a").is_err());
        assert!(parse_pasted_paths("/tmp/a\\").is_err());
        assert!(parse_pasted_paths("./relative").is_err());
        assert_eq!(
            parse_pasted_paths("file://localhost/tmp/a\nfile:///tmp/a").unwrap(),
            vec![PathBuf::from("/tmp/a")]
        );
        assert_eq!(
            parse_pasted_paths("/tmp/'$(rm -rf .)'").unwrap(),
            vec![PathBuf::from("/tmp/$(rm -rf .)")]
        );
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "optionfiles-interaction-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            for name in ["a", "b", "c", "d"] {
                std::fs::write(path.join(name), b"test").unwrap();
            }
            Self(path)
        }
        fn pane(&self) -> Pane {
            let app = App::new(&self.0, false).unwrap();
            Pane {
                location: app.cwd.clone(),
                app: Some(app),
                loading: false,
                error: None,
                receiver: None,
                initial: None,
                range: None,
                refresh_pending: false,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn range_extension_includes_endpoints_and_shrinks_without_erasing_prior_marks() {
        let fixture = Fixture::new();
        let mut pane = fixture.pane();
        let app = pane.app.as_mut().unwrap();
        app.selected = 1;
        app.marked.insert(app.entries[3].path.clone());
        pane.focus(2, true);
        assert_eq!(pane.app.as_ref().unwrap().marked.len(), 3);
        pane.focus(1, true);
        let app = pane.app.as_ref().unwrap();
        assert_eq!(
            app.marked,
            BTreeSet::from([app.entries[1].path.clone(), app.entries[3].path.clone()])
        );
        pane.focus(0, false);
        pane.focus(1, true);
        assert_eq!(pane.app.as_ref().unwrap().marked.len(), 3);
    }

    #[test]
    fn cancelled_filter_restores_original_focus_and_scroll() {
        let fixture = Fixture::new();
        let mut session = Session::new(&fixture.0, false);
        session.panes[0] = fixture.pane();
        let app = session.app_mut().unwrap();
        app.selected = 2;
        app.scroll = 1;
        session.prompt(PromptKind::Search, String::new());
        session.handle_prompt(KeyCode::Char('a'));
        assert_eq!(session.app().unwrap().entries.len(), 1);
        session.handle_prompt(KeyCode::Esc);
        let app = session.app().unwrap();
        assert_eq!(app.current().unwrap().name, "c");
        assert_eq!(app.scroll, 1);
        assert_eq!(app.entries.len(), 4);
    }
}
