use std::io::{self, Write};

use anyhow::Result;
use crossterm::style::Print;
use crossterm::{cursor, queue, terminal};

use crate::preview::PreparedImage;

const IMAGE_ID: u32 = 0x0F11E5;

pub struct KittyPreview {
    supported: bool,
    last_image: Option<u64>,
    last_position: Option<(u16, u16, u16, u16)>,
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
                                || program.eq_ignore_ascii_case("optionterm")
                        })
                        .unwrap_or(false)
            }
        };
        Self {
            supported,
            last_image: None,
            last_position: None,
        }
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    pub fn render(
        &mut self,
        image: Option<&PreparedImage>,
        x: u16,
        y: u16,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        if !self.supported {
            return Ok(());
        }
        let Some(image) = image.filter(|_| cols > 0 && rows > 0) else {
            return self.delete();
        };
        let (cell_width, cell_height) = terminal::window_size()
            .ok()
            .filter(|size| size.columns > 0 && size.rows > 0 && size.width > 0 && size.height > 0)
            .map(|size| {
                (
                    f64::from(size.width) / f64::from(size.columns),
                    f64::from(size.height) / f64::from(size.rows),
                )
            })
            .unwrap_or((8.0, 16.0));
        let (image_cols, image_rows) = placement(
            image.width,
            image.height,
            cols,
            rows,
            cell_width,
            cell_height,
        );
        let x = x.saturating_add(cols.saturating_sub(image_cols) / 2);
        let position = (x, y, image_cols, image_rows);
        if self.last_image == Some(image.id) && self.last_position == Some(position) {
            return Ok(());
        }
        self.delete()?;
        let mut out = io::stdout();
        queue!(out, cursor::MoveTo(x, y))?;
        let mut chunks = image.payload.as_bytes().chunks(4096).peekable();
        let mut first = true;
        while let Some(chunk) = chunks.next() {
            let more = u8::from(chunks.peek().is_some());
            if first {
                queue!(
                    out,
                    Print(format!(
                        "\x1b_Ga=T,f=100,t=d,i={IMAGE_ID},p=1,c={image_cols},r={image_rows},C=1,q=2,m={more};"
                    ))
                )?;
                first = false;
            } else {
                queue!(out, Print(format!("\x1b_Gm={more},q=2;")))?;
            }
            out.write_all(chunk)?;
            out.write_all(b"\x1b\\")?;
        }
        out.flush()?;
        self.last_image = Some(image.id);
        self.last_position = Some(position);
        Ok(())
    }

    pub fn delete(&mut self) -> Result<()> {
        if self.supported && self.last_image.is_some() {
            let mut out = io::stdout();
            queue!(out, Print(format!("\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\")))?;
            out.flush()?;
        }
        self.last_image = None;
        self.last_position = None;
        Ok(())
    }

    pub fn shutdown(&mut self) {
        let _ = self.delete();
        self.supported = false;
    }
}

impl Drop for KittyPreview {
    fn drop(&mut self) {
        let _ = self.delete();
    }
}

fn placement(
    width: u32,
    height: u32,
    cols: u16,
    rows: u16,
    cell_width: f64,
    cell_height: f64,
) -> (u16, u16) {
    let scale = (f64::from(cols) * cell_width / f64::from(width.max(1)))
        .min(f64::from(rows) * cell_height / f64::from(height.max(1)));
    let image_cols = (f64::from(width) * scale / cell_width)
        .round()
        .clamp(1.0, f64::from(cols.max(1))) as u16;
    let image_rows = (f64::from(height) * scale / cell_height)
        .round()
        .clamp(1.0, f64::from(rows.max(1))) as u16;
    (image_cols, image_rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_preserves_image_aspect_ratio_in_cells() {
        assert_eq!(placement(400, 200, 30, 10, 8.0, 16.0), (30, 8));
        assert_eq!(placement(200, 400, 30, 10, 8.0, 16.0), (10, 10));
        assert_eq!(placement(400, 200, 15, 5, 8.0, 16.0), (15, 4));
    }
}
