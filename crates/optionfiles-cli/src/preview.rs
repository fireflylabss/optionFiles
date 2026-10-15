use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine;
use parking_lot::{Condvar, Mutex};

const TEXT_BYTES: usize = 256 * 1024;
const MAX_LINES: usize = 4096;
const OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const CACHE_BYTES: usize = 24 * 1024 * 1024;
const CACHE_ENTRIES: usize = 12;
const CONVERTER_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug)]
pub struct PreparedImage {
    pub width: u32,
    pub height: u32,
    pub(crate) payload: String,
    pub(crate) id: u64,
}

#[derive(Debug)]
pub struct Preview {
    pub kind: &'static str,
    pub lines: Vec<String>,
    pub image: Option<PreparedImage>,
}

#[derive(Debug)]
pub enum PreviewState {
    Empty,
    Loading,
    Ready(Arc<Preview>),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheKey {
    path: PathBuf,
    device: u64,
    inode: u64,
    modified: (i64, i64),
    size: u64,
    cols: u16,
    rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectionKey {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    size: u64,
    cols: u16,
    rows: u16,
}
struct Request {
    key: SelectionKey,
    generation: u64,
}

struct Shared {
    pending: Option<Request>,
    completed: Option<(u64, Result<Arc<Preview>, String>)>,
    stopped: bool,
}

pub struct PreviewService {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    generation: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    key: Option<SelectionKey>,
    state: PreviewState,
    offset: usize,
}

impl PreviewService {
    pub fn new() -> Self {
        let shared = Arc::new((
            Mutex::new(Shared {
                pending: None,
                completed: None,
                stopped: false,
            }),
            Condvar::new(),
        ));
        let generation = Arc::new(AtomicU64::new(0));
        let worker_shared = shared.clone();
        let worker_generation = generation.clone();
        let worker = thread::spawn(move || worker_loop(worker_shared, worker_generation));
        Self {
            shared,
            generation,
            worker: Some(worker),
            key: None,
            state: PreviewState::Empty,
            offset: 0,
        }
    }

    pub fn request_entry(
        &mut self,
        entry: Option<&optionfiles_core::Entry>,
        cols: u16,
        rows: u16,
    ) -> u64 {
        let key = entry
            .filter(|_| cols > 0 && rows > 0)
            .map(|entry| SelectionKey {
                path: entry.path.clone(),
                modified: entry.modified,
                size: entry.size,
                cols,
                rows,
            });
        if self.key == key {
            return self.generation.load(Ordering::Acquire);
        }
        let generation = self
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.key = key.clone();
        self.offset = 0;
        self.state = if key.is_some() {
            PreviewState::Loading
        } else {
            PreviewState::Empty
        };
        let (lock, wake) = &*self.shared;
        let mut shared = lock.lock();
        shared.pending = key.map(|key| Request { key, generation });
        shared.completed = None;
        wake.notify_one();
        generation
    }

    pub fn poll(&mut self) -> bool {
        let completed = self.shared.0.lock().completed.take();
        if let Some((generation, result)) = completed {
            if generation == self.generation.load(Ordering::Acquire) {
                self.state = match result {
                    Ok(preview) => PreviewState::Ready(preview),
                    Err(error) => PreviewState::Error(error),
                };
                return true;
            }
        }
        false
    }

    pub fn state(&self) -> &PreviewState {
        &self.state
    }

    pub fn scroll(&mut self, delta: isize, visible_rows: usize) {
        if let PreviewState::Ready(preview) = &self.state {
            self.offset = self
                .offset
                .saturating_add_signed(delta)
                .min(preview.lines.len().saturating_sub(visible_rows.max(1)));
        }
    }

    pub fn scroll_offset(&self) -> usize {
        self.offset
    }

    pub fn text_lines(&self, visible_rows: usize) -> &[String] {
        match &self.state {
            PreviewState::Ready(preview) => {
                let start = self.offset.min(preview.lines.len());
                &preview.lines[start..start.saturating_add(visible_rows).min(preview.lines.len())]
            }
            _ => &[],
        }
    }
}

impl Drop for PreviewService {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let (lock, wake) = &*self.shared;
        {
            let mut shared = lock.lock();
            shared.stopped = true;
            shared.pending = None;
            wake.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(shared: Arc<(Mutex<Shared>, Condvar)>, generation: Arc<AtomicU64>) {
    let mut cache: VecDeque<(CacheKey, Arc<Preview>, usize)> = VecDeque::new();
    let mut cache_bytes = 0usize;
    loop {
        let request = {
            let (lock, wake) = &*shared;
            let mut state = lock.lock();
            while state.pending.is_none() && !state.stopped {
                wake.wait(&mut state);
            }
            if state.stopped {
                return;
            }
            state.pending.take().expect("pending preview request")
        };
        let context = Context {
            generation: &generation,
            expected: request.generation,
        };
        let result = cache_key(&request.key).and_then(|key| {
            if let Some(index) = cache.iter().position(|(cached, _, _)| cached == &key) {
                let entry = cache.remove(index).expect("cached preview");
                let preview = entry.1.clone();
                cache.push_back(entry);
                return Ok(preview);
            }
            prepare(&key, &context).map(Arc::new).map(|preview| {
                let bytes = preview.lines.iter().map(String::capacity).sum::<usize>()
                    + preview.lines.capacity() * std::mem::size_of::<String>()
                    + preview
                        .image
                        .as_ref()
                        .map_or(0, |image| image.payload.capacity())
                    + std::mem::size_of::<Preview>()
                    + std::mem::size_of::<CacheKey>()
                    + key.path.as_os_str().len()
                    + 2 * std::mem::size_of::<usize>();
                while cache.len() >= CACHE_ENTRIES
                    || cache_bytes.saturating_add(bytes) > CACHE_BYTES
                {
                    let Some((_, _, removed)) = cache.pop_front() else {
                        break;
                    };
                    cache_bytes -= removed;
                }
                if bytes <= CACHE_BYTES && !context.cancelled() {
                    cache_bytes += bytes;
                    cache.push_back((key, preview.clone(), bytes));
                }
                preview
            })
        });
        if !context.cancelled() {
            let mut state = shared.0.lock();
            if !context.cancelled() && !state.stopped {
                state.completed = Some((request.generation, result));
            }
        }
    }
}

fn cache_key(selection: &SelectionKey) -> Result<CacheKey, String> {
    let metadata =
        fs::metadata(&selection.path).map_err(|error| format!("Cannot read preview: {error}"))?;
    Ok(CacheKey {
        path: selection.path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        size: metadata.len(),
        cols: selection.cols,
        rows: selection.rows,
    })
}

struct Context<'a> {
    generation: &'a AtomicU64,
    expected: u64,
}
impl Context<'_> {
    fn cancelled(&self) -> bool {
        self.generation.load(Ordering::Acquire) != self.expected
    }
    fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            Err("Preview cancelled".into())
        } else {
            Ok(())
        }
    }
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Result<Self, String> {
        for _ in 0..16 {
            let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("optionfiles-preview-{}-{id}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("Preview workspace: {error}")),
            }
        }
        Err("Cannot create private preview workspace".into())
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(
    command: &mut Command,
    workspace: &Workspace,
    context: &Context<'_>,
) -> Result<Vec<u8>, String> {
    run_with_limit(command, workspace, context, false)
}

fn run_with_limit(
    command: &mut Command,
    workspace: &Workspace,
    context: &Context<'_>,
    listing: bool,
) -> Result<Vec<u8>, String> {
    context.check()?;
    let helper = command.get_program().to_string_lossy().into_owned();
    let stdout_path = workspace.0.join("stdout");
    let stderr_path = workspace.0.join("stderr");
    let stdout = File::create(&stdout_path).map_err(|e| e.to_string())?;
    let stderr = File::create(&stderr_path).map_err(|e| e.to_string())?;
    command.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            for (resource, limit) in [
                (libc::RLIMIT_AS, 768 * 1024 * 1024),
                (libc::RLIMIT_CPU, 8),
                (libc::RLIMIT_FSIZE, OUTPUT_BYTES as u64),
                (libc::RLIMIT_CORE, 0),
            ] {
                let limits = libc::rlimit {
                    rlim_cur: limit as libc::rlim_t,
                    rlim_max: limit as libc::rlim_t,
                };
                if libc::setrlimit(resource, &limits) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            format!("Missing helper: {helper}")
        } else {
            format!("Cannot start {helper}: {error}")
        }
    })?;
    let started = Instant::now();
    let status = loop {
        if context.cancelled() || started.elapsed() >= CONVERTER_TIMEOUT {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(if context.cancelled() {
                "Preview cancelled".into()
            } else {
                format!("{helper} timed out after 8 seconds")
            });
        }
        if listing
            && fs::metadata(&stdout_path).is_ok_and(|metadata| metadata.len() >= TEXT_BYTES as u64)
        {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return limited_listing(&stdout_path);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{helper}: {error}"));
            }
        }
    };
    // Helpers must not leave grandchildren holding files after their parent exits.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    if listing
        && fs::metadata(&stdout_path).is_ok_and(|metadata| metadata.len() >= TEXT_BYTES as u64)
    {
        return limited_listing(&stdout_path);
    }
    if !status.success() {
        let error = read_prefix(&stderr_path, 4096).unwrap_or_default();
        let detail = sanitize(&String::from_utf8_lossy(&error)).join(" ");
        return Err(if detail.is_empty() {
            format!("{helper} failed ({status})")
        } else {
            format!("{helper}: {detail}")
        });
    }
    read_bounded(&stdout_path, OUTPUT_BYTES)
}

fn read_prefix(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn limited_listing(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = read_prefix(path, TEXT_BYTES)?;
    bytes.extend_from_slice(b"\n[archive output limited to 256 KiB]\n");
    Ok(bytes)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("Preview exceeds {} KiB limit", limit / 1024));
    }
    Ok(bytes)
}

fn prepare(key: &CacheKey, context: &Context<'_>) -> Result<Preview, String> {
    context.check()?;
    let metadata = fs::metadata(&key.path).map_err(|e| format!("Cannot read preview: {e}"))?;
    if metadata.is_dir() {
        return folder_preview(&key.path, context);
    }
    if !metadata.is_file() {
        return Err("No preview for this file type".into());
    }
    let source = key
        .path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve preview: {e}"))?;
    let mut head = [0u8; 4096];
    let count = File::open(&source)
        .map_err(|e| e.to_string())?
        .read(&mut head)
        .map_err(|e| e.to_string())?;
    let head = &head[..count];
    let ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = classify(&ext, head);
    if kind == "text" {
        return text_preview(&source);
    }
    if kind == "binary" {
        return Err("Binary file: no preview available".into());
    }
    let workspace = Workspace::new()?;
    let width = u32::from(key.cols).saturating_mul(8).clamp(8, 1024);
    let height = u32::from(key.rows).saturating_mul(16).clamp(16, 1024);
    let target = workspace.0.join("preview.png");
    match kind {
        "image" => {
            thumbnail(&source, &target, width, height, &workspace, context)?;
            Ok(Preview {
                kind,
                lines: vec![format!("{} bytes", metadata.len())],
                image: Some(prepared_image(&target)?),
            })
        }
        "pdf" => {
            let mut text = Command::new("pdftotext");
            text.args(["-f", "1", "-l", "1", "-layout"])
                .arg(&source)
                .arg("-");
            let text_result = run(&mut text, &workspace, context);
            context.check()?;
            let prefix = workspace.0.join("page");
            let mut raster = Command::new("pdftoppm");
            raster
                .args(["-f", "1", "-l", "1", "-singlefile", "-png", "-scale-to"])
                .arg(width.max(height).to_string())
                .arg(&source)
                .arg(&prefix);
            let image_result = run(&mut raster, &workspace, context)
                .and_then(|_| prepared_image(&prefix.with_extension("png")));
            combine(kind, text_result, image_result, "First page")
        }
        "video" | "audio" => {
            let mut probe = Command::new("ffprobe");
            probe.args(["-v", "error", "-show_entries", "format=duration,format_name:format_tags=title,artist,album,date:stream=codec_name,codec_type,width,height,sample_rate,channels,duration", "-of", "default=noprint_wrappers=1"])
                .arg(&source);
            let metadata_result = run(&mut probe, &workspace, context);
            context.check()?;
            if kind == "audio"
                && metadata_result.as_ref().is_ok_and(|bytes| {
                    !String::from_utf8_lossy(bytes)
                        .lines()
                        .any(|line| line == "codec_type=video")
                })
            {
                let mut lines = sanitize(&String::from_utf8_lossy(
                    metadata_result.as_ref().expect("audio metadata"),
                ));
                lines.push("No embedded cover".into());
                return Ok(Preview {
                    kind,
                    lines,
                    image: None,
                });
            }
            let mut frame = Command::new("ffmpeg");
            frame
                .args(["-v", "error", "-nostdin", "-threads", "1", "-i"])
                .arg(&source);
            if kind == "audio" {
                frame.args(["-map", "0:v:0"]);
            }
            frame
                .args(["-an", "-frames:v", "1", "-vf"])
                .arg(format!(
                    "scale={width}:{height}:force_original_aspect_ratio=decrease"
                ))
                .args(["-threads", "1", "-filter_threads", "1", "-y"])
                .arg(&target);
            let frame_result =
                run(&mut frame, &workspace, context).and_then(|_| prepared_image(&target));
            combine(
                kind,
                metadata_result,
                frame_result,
                if kind == "audio" {
                    "Audio tags / cover"
                } else {
                    "Video frame / metadata"
                },
            )
        }
        "archive" => {
            let mut command = Command::new("bsdtar");
            command.arg("-tf").arg(&source);
            let bytes = run_with_limit(&mut command, &workspace, context, true)?;
            let mut lines = sanitize(&String::from_utf8_lossy(&bytes));
            if lines.len() > 512 {
                lines.truncate(512);
                lines.push("[listing limited to 512 entries]".into());
            }
            if lines.is_empty() {
                lines.push("Empty archive".into());
            }
            Ok(Preview {
                kind,
                lines,
                image: None,
            })
        }
        _ => Err("Binary file: no preview available".into()),
    }
}

fn thumbnail(
    source: &Path,
    target: &Path,
    width: u32,
    height: u32,
    workspace: &Workspace,
    context: &Context<'_>,
) -> Result<(), String> {
    let mut frame = source.as_os_str().to_os_string();
    frame.push("[0]");
    for helper in ["magick", "convert"] {
        let mut command = Command::new(helper);
        command.env("MAGICK_TEMPORARY_PATH", &workspace.0);
        command
            .args([
                "-limit", "memory", "128MiB", "-limit", "map", "128MiB", "-limit", "disk", "32MiB",
                "-limit", "thread", "1",
            ])
            .arg(&frame)
            .arg("-auto-orient")
            .arg("-thumbnail")
            .arg(format!("{width}x{height}>"))
            .arg("-strip")
            .arg(target);
        match run(&mut command, workspace, context) {
            Ok(_) => return Ok(()),
            Err(error) if error == format!("Missing helper: {helper}") => continue,
            Err(error) => return Err(error),
        }
    }
    Err("Missing helper: magick (or convert) for image thumbnails".into())
}

fn combine(
    kind: &'static str,
    text: Result<Vec<u8>, String>,
    image: Result<PreparedImage, String>,
    heading: &str,
) -> Result<Preview, String> {
    if let (Err(text_error), Err(image_error)) = (&text, &image) {
        return Err(format!("{text_error}; {image_error}"));
    }
    let mut lines = vec![heading.into()];
    match text {
        Ok(bytes) => lines.extend(sanitize(&String::from_utf8_lossy(&bytes))),
        Err(error) => lines.push(error),
    }
    let image = match image {
        Ok(image) => Some(image),
        Err(error) => {
            lines.push(error);
            None
        }
    };
    Ok(Preview { kind, lines, image })
}

fn prepared_image(path: &Path) -> Result<PreparedImage, String> {
    let bytes = read_bounded(path, OUTPUT_BYTES)?;
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Converter did not produce a PNG image".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("PNG width"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("PNG height"));
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return Err("Preview image exceeds thumbnail dimensions".into());
    }
    Ok(PreparedImage {
        width,
        height,
        payload: base64::engine::general_purpose::STANDARD.encode(&bytes),
        id: TEMP_ID.fetch_add(1, Ordering::Relaxed),
    })
}

fn folder_preview(path: &Path, context: &Context<'_>) -> Result<Preview, String> {
    let mut children = Vec::new();
    let mut truncated = false;
    for child in fs::read_dir(path)
        .map_err(|e| format!("Cannot list folder: {e}"))?
        .take(129)
    {
        context.check()?;
        let child = child.map_err(|e| e.to_string())?;
        if children.len() == 128 {
            truncated = true;
            break;
        }
        let is_dir = child.file_type().is_ok_and(|kind| kind.is_dir());
        let name = sanitize(&child.file_name().to_string_lossy()).join(" ");
        children.push((!is_dir, format!("{name}{}", if is_dir { "/" } else { "" })));
    }
    children.sort_unstable();
    let mut lines: Vec<String> = children.into_iter().map(|(_, name)| name).collect();
    if truncated {
        lines.push("[folder preview limited to 128 children]".into());
    }
    if lines.is_empty() {
        lines.push("Empty folder".into());
    }
    Ok(Preview {
        kind: "folder",
        lines,
        image: None,
    })
}

fn text_preview(path: &Path) -> Result<Preview, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take((TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let truncated = bytes.len() > TEXT_BYTES;
    bytes.truncate(TEXT_BYTES);
    if bytes.contains(&0) {
        return Err("Binary file: no text preview".into());
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])
                .map_err(|_| "Not UTF-8 text".to_string())?
        }
        Err(_) => return Err("Not UTF-8 text: no text preview".into()),
    };
    let mut lines = sanitize(text);
    if truncated {
        lines.push("[text preview limited to 256 KiB]".into());
    }
    if lines.is_empty() {
        lines.push("Empty file".into());
    }
    Ok(Preview {
        kind: "text",
        lines,
        image: None,
    })
}

fn classify(ext: &str, head: &[u8]) -> &'static str {
    if head.starts_with(b"%PDF-") || ext == "pdf" {
        return "pdf";
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(b"\xff\xd8\xff")
        || head.starts_with(b"GIF8")
        || matches!(
            ext,
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "tif"
                | "tiff"
                | "avif"
                | "heic"
                | "ico"
                | "svg"
        )
    {
        return "image";
    }
    if matches!(
        ext,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "mpeg" | "mpg" | "wmv" | "ogv"
    ) || (head.first() == Some(&0x47) && head.get(188) == Some(&0x47))
    {
        return "video";
    }
    if head.starts_with(b"ID3")
        || head.starts_with(b"fLaC")
        || matches!(
            ext,
            "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "aiff" | "wma"
        )
    {
        return "audio";
    }
    if head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"7z\xbc\xaf\x27\x1c")
        || head.starts_with(b"Rar!")
        || matches!(
            ext,
            "zip"
                | "tar"
                | "gz"
                | "bz2"
                | "xz"
                | "zst"
                | "7z"
                | "rar"
                | "tgz"
                | "tbz"
                | "txz"
                | "jar"
                | "apk"
                | "deb"
                | "rpm"
                | "iso"
        )
    {
        return "archive";
    }
    if head.contains(&0) { "binary" } else { "text" }
}

pub(crate) fn sanitize(text: &str) -> Vec<String> {
    #[derive(Clone, Copy)]
    enum Escape {
        None,
        Start,
        Csi,
        String,
        StringEscape,
    }
    let mut escape = Escape::None;
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut chars = 0usize;
    let mut clipped = false;
    for ch in text.chars() {
        match escape {
            Escape::Start => {
                escape = match ch {
                    '[' => Escape::Csi,
                    ']' | 'P' | '^' | '_' => Escape::String,
                    _ => Escape::None,
                };
                continue;
            }
            Escape::Csi => {
                if ('@'..='~').contains(&ch) {
                    escape = Escape::None;
                }
                continue;
            }
            Escape::String => {
                if ch == '\x07' || ch == '\u{009c}' {
                    escape = Escape::None;
                } else if ch == '\x1b' {
                    escape = Escape::StringEscape;
                }
                continue;
            }
            Escape::StringEscape => {
                escape = if ch == '\\' {
                    Escape::None
                } else {
                    Escape::String
                };
                continue;
            }
            Escape::None => {}
        }
        if ch == '\x1b' {
            escape = Escape::Start;
            continue;
        }
        if ch == '\u{009b}' {
            escape = Escape::Csi;
            continue;
        }
        if matches!(ch, '\u{0090}' | '\u{009d}' | '\u{009e}' | '\u{009f}') {
            escape = Escape::String;
            continue;
        }
        if ch == '\n' {
            if clipped {
                line.push_str(" [line clipped]");
            }
            lines.push(std::mem::take(&mut line));
            chars = 0;
            clipped = false;
            if lines.len() >= MAX_LINES {
                lines.push("[preview limited to 4096 lines]".into());
                return lines;
            }
        } else if ch == '\t' {
            for _ in 0..4 {
                if chars < 4096 {
                    line.push(' ');
                    chars += 1;
                } else {
                    clipped = true;
                }
            }
        } else if !ch.is_control()
            && !matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            if chars < 4096 {
                line.push(ch);
                chars += 1;
            } else {
                clipped = true;
            }
        }
    }
    if clipped {
        line.push_str(" [line clipped]");
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_terminal_control_sequences() {
        assert_eq!(
            sanitize("before\x1b[31mred\x1b[0m\x1b]52;c;clipboard\x07after\r\n\tend"),
            vec!["beforeredafter", "    end"]
        );
        assert_eq!(
            sanitize("safe\u{009b}31mtext\u{202e}done"),
            vec!["safetextdone"]
        );
    }

    #[test]
    fn recognizes_extensionless_content() {
        assert_eq!(classify("", b"#!/bin/sh\necho hi"), "text");
        assert_eq!(classify("", b"%PDF-1.4"), "pdf");
        assert_eq!(classify("", b"\x89PNG\r\n\x1a\n"), "image");
    }

    #[test]
    fn typescript_source_is_not_mpeg_transport_stream() {
        assert_eq!(classify("ts", b"export const selected = true;"), "text");
        let mut transport = [0u8; 376];
        transport[0] = 0x47;
        transport[188] = 0x47;
        assert_eq!(classify("ts", &transport), "video");
    }

    #[test]
    fn clips_long_text_lines() {
        let lines = sanitize(&"a".repeat(10000));
        assert_eq!(lines[0].len(), 4096 + " [line clipped]".len());
    }

    #[test]
    fn scroll_is_bounded_by_visible_window() {
        let mut service = PreviewService::new();
        service.state = PreviewState::Ready(Arc::new(Preview {
            kind: "text",
            lines: (0..10).map(|i| i.to_string()).collect(),
            image: None,
        }));
        service.scroll(100, 3);
        assert_eq!(service.scroll_offset(), 7);
        assert_eq!(service.text_lines(3), &["7", "8", "9"]);
        service.scroll(-100, 3);
        assert_eq!(service.scroll_offset(), 0);
    }

    #[test]
    fn rejects_stale_worker_results() {
        let mut service = PreviewService::new();
        service.generation.store(2, Ordering::Release);
        service.state = PreviewState::Loading;
        service.shared.0.lock().completed = Some((1, Err("stale error".into())));
        assert!(!service.poll());
        assert!(matches!(service.state(), PreviewState::Loading));
        service.shared.0.lock().completed = Some((2, Err("current error".into())));
        assert!(service.poll());
        assert!(matches!(service.state(), PreviewState::Error(error) if error == "current error"));
    }
}
