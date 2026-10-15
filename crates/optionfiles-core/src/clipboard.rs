use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    pub mode: ClipboardMode,
}

pub const URI_LIST: &str = "text/uri-list";
pub const GNOME_COPIED_FILES: &str = "x-special/gnome-copied-files";
pub const KDE_CUT_SELECTION: &str = "application/x-kde-cutselection";

pub fn clipboard_encode(clipboard: &Clipboard, mime: &str) -> Vec<u8> {
    if mime == KDE_CUT_SELECTION {
        return if clipboard.mode == ClipboardMode::Cut {
            b"1"
        } else {
            b"0"
        }
        .to_vec();
    }
    let mut text = String::new();
    if mime == GNOME_COPIED_FILES {
        text.push_str(if clipboard.mode == ClipboardMode::Cut {
            "cut\n"
        } else {
            "copy\n"
        });
    }
    for path in &clipboard.paths {
        text.push_str(&path_uri(path));
        text.push_str(if mime == URI_LIST { "\r\n" } else { "\n" });
    }
    text.into_bytes()
}

pub fn clipboard_decode(bytes: &[u8], mime: &str) -> Result<Clipboard> {
    let text = std::str::from_utf8(bytes)?.trim_matches('\0');
    let mut lines = text.lines();
    let mode = if mime == GNOME_COPIED_FILES {
        match lines.next().map(str::trim) {
            Some("cut") => ClipboardMode::Cut,
            Some("copy") => ClipboardMode::Copy,
            _ => bail!("invalid GNOME file clipboard operation"),
        }
    } else if mime == URI_LIST {
        ClipboardMode::Copy
    } else {
        bail!("unsupported file clipboard MIME type");
    };
    let mut paths = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = parse_file_uri(line)?;
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        bail!("file clipboard is empty");
    }
    Ok(Clipboard { paths, mode })
}

pub fn clipboard_export(clipboard: &Clipboard) -> Result<Option<&'static str>> {
    if clipboard.paths.is_empty() {
        return Ok(None);
    }
    let payload = clipboard_encode(clipboard, GNOME_COPIED_FILES);
    for (program, args) in [
        ("wl-copy", vec!["--type", GNOME_COPIED_FILES]),
        (
            "xclip",
            vec!["-selection", "clipboard", "-t", GNOME_COPIED_FILES],
        ),
    ] {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let written = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(&payload).is_ok());
        let success = child.wait().is_ok_and(|status| status.success());
        if written && success {
            return Ok(Some(program));
        }
    }
    Ok(None)
}

fn read_type(program: &str, mime: &str) -> Option<Vec<u8>> {
    let args = if program == "wl-paste" {
        vec!["--no-newline", "--type", mime]
    } else {
        vec!["-selection", "clipboard", "-o", "-t", mime]
    };
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

pub fn clipboard_import() -> Option<Clipboard> {
    for program in ["wl-paste", "xclip"] {
        for mime in [GNOME_COPIED_FILES, URI_LIST] {
            let Some(bytes) = read_type(program, mime) else {
                continue;
            };
            let Ok(mut clipboard) = clipboard_decode(&bytes, mime) else {
                continue;
            };
            if mime == URI_LIST
                && read_type(program, KDE_CUT_SELECTION)
                    .is_some_and(|bytes| bytes.first() == Some(&b'1'))
            {
                clipboard.mode = ClipboardMode::Cut;
            }
            return Some(clipboard);
        }
    }
    None
}

pub fn path_uri(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        absolute.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let text = absolute.to_string_lossy();
    #[cfg(not(unix))]
    let bytes = text.as_bytes();
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() + 7);
    out.push_str("file://");
    for &byte in bytes {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 15) as usize] as char);
            }
        }
    }
    out
}

pub fn parse_file_uri(uri: &str) -> Result<PathBuf> {
    let rest = uri.strip_prefix("file://").context("not a file URI")?;
    let path = if rest.starts_with('/') {
        rest
    } else if let Some(path) = rest.strip_prefix("localhost/") {
        // Retain the leading slash after the authority.
        &rest[rest.len() - path.len() - 1..]
    } else {
        bail!("remote file URI is not supported");
    };
    if path.contains(['?', '#', '\0', '\r', '\n']) {
        bail!("invalid unescaped character in file URI");
    }
    let mut decoded = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = bytes
                .get(index + 1..index + 3)
                .context("truncated URI escape")?;
            let high = hex(digits[0]).context("invalid URI escape")?;
            let low = hex(digits[1]).context("invalid URI escape")?;
            let byte = high * 16 + low;
            if byte == 0 {
                bail!("file URI contains NUL");
            }
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
    }
    #[cfg(not(unix))]
    Ok(PathBuf::from(String::from_utf8(decoded)?))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
