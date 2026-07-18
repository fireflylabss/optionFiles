//! Image previews using the Kitty terminal graphics protocol.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use crossterm::style::Print;
use crossterm::{cursor, queue};

const IMAGE_NUMBER: u32 = 0x0F11E5;

pub struct KittyPreview {
    supported: bool,
    last_source: Option<PathBuf>,
    converted: Option<PathBuf>,
}

impl KittyPreview {
    pub fn detect() -> Self {
        let forced = std::env::var("OPTIONFILES_KITTY_GRAPHICS").ok();
        let supported = match forced.as_deref() {
            Some("1" | "true" | "yes") => true,
            Some("0" | "false" | "no") => false,
            _ => {
                std::env::var_os("KITTY_WINDOW_ID").is_some()
                    || std::env::var("TERM")
                        .map(|term| term.contains("kitty") || term.contains("ghostty"))
                        .unwrap_or(false)
                    || std::env::var("TERM_PROGRAM")
                        .map(|program| {
                            program.eq_ignore_ascii_case("wezterm")
                                || program.eq_ignore_ascii_case("ghostty")
                        })
                        .unwrap_or(false)
            }
        };
        Self {
            supported,
            last_source: None,
            converted: None,
        }
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    pub fn can_preview(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff"
                )
            })
            .unwrap_or(false)
    }

    pub fn render(
        &mut self,
        source: Option<&Path>,
        x: u16,
        y: u16,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        if !self.supported || cols == 0 || rows == 0 {
            return Ok(());
        }
        self.delete()?;
        let Some(source) = source.filter(|path| Self::can_preview(path)) else {
            self.last_source = None;
            return Ok(());
        };

        let image = if source
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        {
            source.to_path_buf()
        } else if self.last_source.as_deref() == Some(source) {
            match self.converted.as_ref().filter(|path| path.is_file()) {
                Some(converted) => converted.clone(),
                None => return Ok(()),
            }
        } else if let Some(converted) = convert_to_png(source) {
            if let Some(old) = self.converted.replace(converted.clone()) {
                let _ = fs::remove_file(old);
            }
            converted
        } else {
            self.last_source = None;
            return Ok(());
        };

        let absolute = image.canonicalize().unwrap_or(image);
        let payload = base64(absolute.to_string_lossy().as_bytes());
        let command = format!(
            "\x1b_Ga=T,f=100,t=f,I={IMAGE_NUMBER},p=1,c={cols},r={rows},C=1,q=2;{payload}\x1b\\"
        );
        let mut out = io::stdout();
        queue!(out, cursor::MoveTo(x, y), Print(command))?;
        out.flush()?;
        self.last_source = Some(source.to_path_buf());
        Ok(())
    }

    pub fn delete(&mut self) -> Result<()> {
        if self.supported {
            let mut out = io::stdout();
            queue!(
                out,
                Print(format!("\x1b_Ga=d,d=N,I={IMAGE_NUMBER},p=1,q=2\x1b\\"))
            )?;
            out.flush()?;
        }
        Ok(())
    }

    pub fn shutdown(&mut self) {
        let _ = self.delete();
        self.supported = false;
        if let Some(path) = self.converted.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for KittyPreview {
    fn drop(&mut self) {
        let _ = self.delete();
        if let Some(path) = self.converted.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn convert_to_png(source: &Path) -> Option<PathBuf> {
    let target = std::env::temp_dir().join(format!(
        "optionfiles-tty-graphics-protocol-{}.png",
        std::process::id()
    ));
    let source_frame = format!("{}[0]", source.display());
    for program in ["magick", "convert"] {
        let status = Command::new(program)
            .arg(&source_frame)
            .arg(&target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) && target.is_file() {
            return Some(target);
        }
    }
    None
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b"kitty"), "a2l0dHk=");
        assert_eq!(base64(b"png"), "cG5n");
    }

    #[test]
    fn recognizes_image_extensions() {
        assert!(KittyPreview::can_preview(Path::new("cover.PNG")));
        assert!(KittyPreview::can_preview(Path::new("photo.jpeg")));
        assert!(!KittyPreview::can_preview(Path::new("notes.md")));
    }
}
