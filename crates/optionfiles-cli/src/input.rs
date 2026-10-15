use std::collections::VecDeque;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

const MAX_FRAME: usize = 1024 * 1024;
const ESCAPE_DELAY: Duration = Duration::from_millis(30);

pub enum InputEvent {
    Terminal(Event),
    Osc(String),
    PrimaryAttributes(Vec<u16>),
    Error(String),
}

pub struct Input {
    decoder: Decoder,
}

impl Input {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(),
        }
    }

    pub fn poll(&mut self, timeout: Duration) -> io::Result<Option<InputEvent>> {
        if let Some(event) = self.decoder.next() {
            return Ok(Some(event));
        }
        let stdin = io::stdin();
        let mut descriptor = libc::pollfd {
            fd: stdin.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
        // Reading here, rather than on a detached thread, leaves stdin available to $EDITOR.
        let ready = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(self.decoder.next());
            }
            return Err(error);
        }
        if ready == 0 {
            return Ok(self.decoder.next());
        }
        let mut bytes = [0; 8192];
        let count = stdin.lock().read(&mut bytes)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal input closed",
            ));
        }
        self.decoder.feed(&bytes[..count]);
        Ok(self.decoder.next())
    }
}

struct Decoder {
    bytes: VecDeque<u8>,
    frame: Vec<u8>,
    mode: Mode,
    last_byte: Instant,
    overflow: bool,
    pending_alt: bool,
}
#[derive(PartialEq)]
enum Mode {
    Normal,
    Osc,
    Paste,
    Csi,
    Ss3,
    Ignore,
}

impl Decoder {
    fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
            frame: Vec::new(),
            mode: Mode::Normal,
            last_byte: Instant::now(),
            overflow: false,
            pending_alt: false,
        }
    }
    fn feed(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        self.last_byte = Instant::now();
    }
    fn starts(&self, prefix: &[u8]) -> bool {
        self.bytes
            .iter()
            .copied()
            .take(prefix.len())
            .eq(prefix.iter().copied())
            && self.bytes.len() >= prefix.len()
    }
    fn consume(&mut self, count: usize) -> Vec<u8> {
        self.bytes.drain(..count).collect()
    }
    fn key(code: KeyCode, modifiers: KeyModifiers) -> InputEvent {
        InputEvent::Terminal(Event::Key(KeyEvent::new(code, modifiers)))
    }
    fn finish_key(&mut self, code: KeyCode, mut modifiers: KeyModifiers) -> InputEvent {
        if std::mem::take(&mut self.pending_alt) {
            modifiers |= KeyModifiers::ALT;
        }
        Self::key(code, modifiers)
    }
    fn append_frame(&mut self, byte: u8) {
        if self.frame.len() < MAX_FRAME {
            self.frame.push(byte);
        } else {
            self.overflow = true;
        }
    }

    fn next(&mut self) -> Option<InputEvent> {
        loop {
            if self.mode == Mode::Ignore {
                let byte = self.bytes.pop_front()?;
                if byte == 0x1b {
                    if self.bytes.front() == Some(&b'\\') {
                        self.bytes.pop_front();
                        self.mode = Mode::Normal;
                        self.pending_alt = false;
                    } else if self.bytes.is_empty() {
                        self.bytes.push_front(byte);
                        return None;
                    }
                }
                continue;
            }
            if self.mode == Mode::Osc {
                let byte = self.bytes.pop_front()?;
                if byte == 7 || (byte == 0x1b && self.bytes.front() == Some(&b'\\')) {
                    if byte == 0x1b {
                        self.bytes.pop_front();
                    }
                    self.mode = Mode::Normal;
                    self.pending_alt = false;
                    if std::mem::take(&mut self.overflow) {
                        self.frame.clear();
                        return Some(InputEvent::Error("terminal OSC exceeded 1 MiB".into()));
                    }
                    let frame = std::mem::take(&mut self.frame);
                    return Some(match String::from_utf8(frame) {
                        Ok(frame) => InputEvent::Osc(frame),
                        Err(_) => InputEvent::Error("terminal OSC contains invalid UTF-8".into()),
                    });
                }
                if byte == 0x1b && self.bytes.is_empty() {
                    self.bytes.push_front(byte);
                    return None;
                }
                self.append_frame(byte);
                continue;
            }
            if self.mode == Mode::Paste {
                let end = b"\x1b[201~";
                if self.starts(end) {
                    self.consume(end.len());
                    self.mode = Mode::Normal;
                    self.pending_alt = false;
                    if std::mem::take(&mut self.overflow) {
                        self.frame.clear();
                        return Some(InputEvent::Error("pasted input exceeded 1 MiB".into()));
                    }
                    return Some(InputEvent::Terminal(Event::Paste(
                        String::from_utf8_lossy(&std::mem::take(&mut self.frame)).into_owned(),
                    )));
                }
                if self.bytes.len() < end.len()
                    && self
                        .bytes
                        .iter()
                        .copied()
                        .eq(end[..self.bytes.len()].iter().copied())
                {
                    return None;
                }
                let byte = self.bytes.pop_front()?;
                self.append_frame(byte);
                continue;
            }
            if self.mode == Mode::Csi || self.mode == Mode::Ss3 {
                let byte = self.bytes.pop_front()?;
                if (0x40..=0x7e).contains(&byte) {
                    let mode = std::mem::replace(&mut self.mode, Mode::Normal);
                    if std::mem::take(&mut self.overflow) {
                        self.frame.clear();
                        self.pending_alt = false;
                        return Some(InputEvent::Error("oversized terminal key sequence".into()));
                    }
                    self.frame.push(byte);
                    let sequence = std::mem::take(&mut self.frame);
                    if mode == Mode::Csi && sequence == b"200~" {
                        self.mode = Mode::Paste;
                        self.pending_alt = false;
                        continue;
                    }
                    let event = if mode == Mode::Csi {
                        parse_csi(&sequence)
                    } else {
                        parse_ss3(&sequence)
                    };
                    let alt = std::mem::take(&mut self.pending_alt);
                    if let Some(mut event) = event {
                        if let InputEvent::Terminal(Event::Key(key)) = &mut event {
                            if alt {
                                key.modifiers |= KeyModifiers::ALT;
                            }
                        }
                        return Some(event);
                    }
                    continue;
                }
                if self.frame.len() < 128 {
                    self.frame.push(byte);
                } else {
                    self.overflow = true;
                }
                continue;
            }
            let byte = *self.bytes.front()?;
            if byte == 0x1b {
                if self.bytes.len() == 1 {
                    if self.last_byte.elapsed() < ESCAPE_DELAY {
                        return None;
                    }
                    self.bytes.pop_front();
                    return Some(self.finish_key(KeyCode::Esc, KeyModifiers::NONE));
                }
                if self.starts(b"\x1b]") {
                    self.consume(2);
                    self.mode = Mode::Osc;
                    continue;
                }
                if self.starts(b"\x1bP") || self.starts(b"\x1b_") || self.starts(b"\x1b^") {
                    self.consume(2);
                    self.mode = Mode::Ignore;
                    continue;
                }
                if self.starts(b"\x1b[") {
                    self.consume(2);
                    self.mode = Mode::Csi;
                    continue;
                }
                if self.starts(b"\x1bO") {
                    self.consume(2);
                    self.mode = Mode::Ss3;
                    continue;
                }
                self.bytes.pop_front();
                if self.pending_alt {
                    return Some(self.finish_key(KeyCode::Esc, KeyModifiers::NONE));
                }
                self.pending_alt = true;
                continue;
            }
            let code = match byte {
                b'\r' | b'\n' => KeyCode::Enter,
                b'\t' => KeyCode::Tab,
                0x7f | 8 => KeyCode::Backspace,
                1..=26 => {
                    self.bytes.pop_front();
                    return Some(self.finish_key(
                        KeyCode::Char((b'a' + byte - 1) as char),
                        KeyModifiers::CONTROL,
                    ));
                }
                0 => {
                    self.bytes.pop_front();
                    return Some(self.finish_key(KeyCode::Char(' '), KeyModifiers::CONTROL));
                }
                0x1c..=0x1f => {
                    self.bytes.pop_front();
                    return Some(self.finish_key(
                        KeyCode::Char((b'\\' + byte - 0x1c) as char),
                        KeyModifiers::CONTROL,
                    ));
                }
                0x20..=0x7e => KeyCode::Char(byte as char),
                _ => {
                    let width = match byte {
                        0xc2..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf4 => 4,
                        _ => 0,
                    };
                    if width == 0 {
                        self.bytes.pop_front();
                        self.pending_alt = false;
                        continue;
                    }
                    if self
                        .bytes
                        .iter()
                        .skip(1)
                        .take(width - 1)
                        .any(|b| b & 0xc0 != 0x80)
                    {
                        self.bytes.pop_front();
                        self.pending_alt = false;
                        continue;
                    }
                    if self.bytes.len() < width {
                        return None;
                    }
                    let mut utf8 = [0; 4];
                    for (target, source) in utf8.iter_mut().zip(self.bytes.iter()).take(width) {
                        *target = *source;
                    }
                    let Ok(text) = std::str::from_utf8(&utf8[..width]) else {
                        self.bytes.pop_front();
                        self.pending_alt = false;
                        continue;
                    };
                    let code = KeyCode::Char(text.chars().next()?);
                    self.consume(width);
                    return Some(self.finish_key(code, KeyModifiers::NONE));
                }
            };
            self.bytes.pop_front();
            return Some(self.finish_key(code, KeyModifiers::NONE));
        }
    }
}

fn modifiers(value: u16) -> KeyModifiers {
    let value = value.saturating_sub(1);
    let mut modifiers = KeyModifiers::NONE;
    if value & 1 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if value & 2 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if value & 4 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    modifiers
}

fn parse_ss3(sequence: &[u8]) -> Option<InputEvent> {
    let final_byte = *sequence.last()?;
    let args = std::str::from_utf8(&sequence[..sequence.len() - 1]).ok()?;
    let modifier = if args.is_empty() {
        KeyModifiers::NONE
    } else {
        modifiers(args.split(';').nth(1)?.parse().ok()?)
    };
    let code = match final_byte {
        b'P' => KeyCode::F(1),
        b'Q' => KeyCode::F(2),
        b'R' => KeyCode::F(3),
        b'S' => KeyCode::F(4),
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'H' => KeyCode::Home,
        b'F' => KeyCode::End,
        _ => return None,
    };
    Some(Decoder::key(code, modifier))
}

fn parse_csi(sequence: &[u8]) -> Option<InputEvent> {
    let final_byte = *sequence.last()?;
    let args = std::str::from_utf8(&sequence[..sequence.len() - 1]).ok()?;
    if let Some(args) = args.strip_prefix('?') {
        if final_byte != b'c' {
            return None;
        }
        let attrs = args
            .split(';')
            .map(str::parse)
            .collect::<Result<Vec<u16>, _>>()
            .ok()?;
        return Some(InputEvent::PrimaryAttributes(attrs));
    }
    if let Some(args) = args.strip_prefix('<') {
        if !matches!(final_byte, b'M' | b'm') {
            return None;
        }
        let values = args
            .split(';')
            .map(str::parse)
            .collect::<Result<Vec<u16>, _>>()
            .ok()?;
        if values.len() != 3 || values[1] == 0 || values[2] == 0 {
            return None;
        }
        let button = values[0];
        let mut modifiers = KeyModifiers::NONE;
        if button & 4 != 0 {
            modifiers |= KeyModifiers::SHIFT;
        }
        if button & 8 != 0 {
            modifiers |= KeyModifiers::ALT;
        }
        if button & 16 != 0 {
            modifiers |= KeyModifiers::CONTROL;
        }
        let mouse_button = match button & 3 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            _ => MouseButton::Right,
        };
        let kind = if button & 64 != 0 {
            match button & 3 {
                0 => MouseEventKind::ScrollUp,
                1 => MouseEventKind::ScrollDown,
                2 => MouseEventKind::ScrollLeft,
                _ => MouseEventKind::ScrollRight,
            }
        } else if final_byte == b'm' {
            MouseEventKind::Up(mouse_button)
        } else if button & 32 != 0 {
            if button & 3 == 3 {
                MouseEventKind::Moved
            } else {
                MouseEventKind::Drag(mouse_button)
            }
        } else {
            MouseEventKind::Down(mouse_button)
        };
        return Some(InputEvent::Terminal(Event::Mouse(MouseEvent {
            kind,
            column: values[1] - 1,
            row: values[2] - 1,
            modifiers,
        })));
    }
    if final_byte == b'c' {
        return None;
    }
    let values = args
        .split(';')
        .map(|v| {
            if v.is_empty() {
                Ok(1)
            } else {
                v.parse::<u16>()
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() > 2 {
        return None;
    }
    let modifier = values
        .get(1)
        .copied()
        .map(modifiers)
        .unwrap_or(KeyModifiers::NONE);
    let code = match final_byte {
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'H' => KeyCode::Home,
        b'F' => KeyCode::End,
        b'Z' => {
            return Some(Decoder::key(
                KeyCode::BackTab,
                modifier | KeyModifiers::SHIFT,
            ));
        }
        b'P' => KeyCode::F(1),
        b'Q' => KeyCode::F(2),
        b'R' => KeyCode::F(3),
        b'S' => KeyCode::F(4),
        b'~' => match values[0] {
            1 | 7 => KeyCode::Home,
            2 => KeyCode::Insert,
            3 => KeyCode::Delete,
            4 | 8 => KeyCode::End,
            5 => KeyCode::PageUp,
            6 => KeyCode::PageDown,
            11..=15 => KeyCode::F((values[0] - 10) as u8),
            17..=21 => KeyCode::F((values[0] - 11) as u8),
            23..=24 => KeyCode::F((values[0] - 12) as u8),
            _ => return None,
        },
        _ => return None,
    };
    Some(Decoder::key(code, modifier))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interleaved_osc_paste_and_keys_are_separate() {
        let mut parser = Decoder::new();
        parser.feed(b"j\x1b]72;t=M:x=1:y=2\x1b\\\x1b[200~q\n/tmp/a\x1b[201~k");
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('j'),
                ..
            })))
        ));
        assert!(matches!(parser.next(), Some(InputEvent::Osc(frame)) if frame == "72;t=M:x=1:y=2"));
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Paste(value))) if value == "q\n/tmp/a")
        );
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('k'),
                ..
            })))
        ));
    }
    #[test]
    fn sgr_mouse_coordinates_are_zero_based() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b[<32;11;7M");
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: 6,
                ..
            })))
        ));
    }
    #[test]
    fn split_osc_terminator_is_not_a_shortcut() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b]72;test\x1b");
        assert!(parser.next().is_none());
        parser.feed(b"\\q");
        assert!(matches!(parser.next(), Some(InputEvent::Osc(frame)) if frame == "72;test"));
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            })))
        ));
    }
    #[test]
    fn utf8_and_alt_survive_split_reads() {
        let mut parser = Decoder::new();
        parser.feed(&[0x1b, 0xc3]);
        assert!(parser.next().is_none());
        parser.feed(&[0xa9]);
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::Char('é'), modifiers, .. }))) if modifiers == KeyModifiers::ALT)
        );
        parser.feed(&[0xe6, 0x97]);
        assert!(parser.next().is_none());
        parser.feed(&[0xa5]);
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('日'),
                ..
            })))
        ));
    }
    #[test]
    fn escape_key_waits_without_sleeping() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b");
        assert!(parser.next().is_none());
        parser.last_byte = Instant::now() - ESCAPE_DELAY - Duration::from_millis(1);
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Esc,
                ..
            })))
        ));
    }
    #[test]
    fn oversized_osc_discards_embedded_shortcuts_until_terminator() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b]72;");
        parser.feed(&vec![b'q'; MAX_FRAME + 1]);
        parser.feed(b"\x1b\\j");
        assert!(matches!(parser.next(), Some(InputEvent::Error(_))));
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('j'),
                ..
            })))
        ));
    }
    #[test]
    fn paste_preserves_spaces_unicode_newlines_and_control_bytes_as_data() {
        let mut parser = Decoder::new();
        parser.feed("\x1b[200~/tmp/a b\n/tmp/日本\nq\u{3}\x1b[201~".as_bytes());
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Paste(value))) if value == "/tmp/a b\n/tmp/日本\nq\u{3}")
        );
        parser.feed(b"\x03");
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, .. }))) if modifiers == KeyModifiers::CONTROL)
        );
    }
    #[test]
    fn osc_chunks_and_mouse_modifiers_interleave_with_keys() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b]72;t=r:m=1;ab");
        assert!(parser.next().is_none());
        parser.feed(b"cd\x07k\x1b]72;t=r:m=0;\x07\x1b[<20;3;6M");
        assert!(
            matches!(parser.next(), Some(InputEvent::Osc(frame)) if frame == "72;t=r:m=1;abcd")
        );
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('k'),
                ..
            })))
        ));
        assert!(matches!(parser.next(), Some(InputEvent::Osc(frame)) if frame == "72;t=r:m=0;"));
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Mouse(MouseEvent { modifiers, column: 2, row: 5, .. }))) if modifiers == KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        );
    }
    #[test]
    fn protocol_frames_survive_every_read_boundary() {
        let cases: &[(&[u8], &str)] = &[
            (b"\x1b]72;t=M:x=2:y=5;o=1\x1b\\", "osc"),
            (b"\x1b]72;t=q\x07", "osc"),
            (b"\x1b[200~/tmp/a\x1b]72;t=M\x07\nq\x1b[201~", "paste"),
            (b"\x1b[?62;1;4;22c", "attributes"),
            (b"\x1b[1;6A", "key"),
            (b"\x1b[<20;9;3M", "mouse"),
            ("\x1bé".as_bytes(), "key"),
        ];
        for &(frame, kind) in cases {
            for boundary in 0..=frame.len() {
                let mut parser = Decoder::new();
                parser.feed(&frame[..boundary]);
                let first = parser.next();
                parser.feed(&frame[boundary..]);
                let event = first.or_else(|| parser.next()).expect("one complete event");
                assert!(
                    match (kind, event) {
                        ("osc", InputEvent::Osc(_)) => true,
                        ("paste", InputEvent::Terminal(Event::Paste(value))) =>
                            value.contains("\x1b]72;t=M"),
                        ("attributes", InputEvent::PrimaryAttributes(values)) =>
                            values == [62, 1, 4, 22],
                        ("key", InputEvent::Terminal(Event::Key(_))) => true,
                        ("mouse", InputEvent::Terminal(Event::Mouse(_))) => true,
                        _ => false,
                    },
                    "frame type changed at byte {boundary}"
                );
                assert!(parser.next().is_none());
            }
        }
    }

    #[test]
    fn oversized_keys_discard_to_their_final_byte_not_the_next_read() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b[");
        parser.feed(&vec![b'1'; 256]);
        assert!(parser.next().is_none());
        parser.feed(b";999999Aq");
        assert!(matches!(parser.next(), Some(InputEvent::Error(_))));
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers, .. }))) if modifiers.is_empty())
        );
    }

    #[test]
    fn oversized_paste_never_replays_shortcuts() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b[200~");
        parser.feed(&vec![b'q'; MAX_FRAME + 1]);
        assert!(parser.next().is_none());
        parser.feed(b"\x1b[201~j");
        assert!(matches!(parser.next(), Some(InputEvent::Error(_))));
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('j'),
                ..
            })))
        ));
    }

    #[test]
    fn invalid_utf8_does_not_consume_ascii_control_or_next_sequence() {
        let mut parser = Decoder::new();
        parser.feed(b"\xc3q\xe6\x03\xf0\x1b[A");
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            })))
        ));
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, .. }))) if modifiers == KeyModifiers::CONTROL)
        );
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Up,
                ..
            })))
        ));
    }

    #[test]
    fn malformed_protocol_parameters_do_not_turn_into_navigation() {
        assert!(parse_csi(b"1:badA").is_none());
        assert!(parse_csi(b"999999999999~").is_none());
        assert!(parse_csi(b"<0;0;3M").is_none());
        assert!(parse_csi(b"<0;2;3A").is_none());
        assert!(parse_csi(b"?1;badc").is_none());
        let mut parser = Decoder::new();
        parser.feed(b"\x1b[999999999;1A\x1b_Gq;rm -rf /");
        assert!(parser.next().is_none());
        parser.feed(b"\x1b\\j");
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('j'),
                ..
            })))
        ));
    }

    #[test]
    fn modifiers_and_function_keys_keep_their_identity() {
        let mut parser = Decoder::new();
        parser.feed(b"\x1b[1;6A\x1b[15~\x1b[17~\x1b[Z\x1b\x01\x1bO1;5P");
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::Up, modifiers, .. }))) if modifiers == KeyModifiers::SHIFT | KeyModifiers::CONTROL)
        );
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::F(5),
                ..
            })))
        ));
        assert!(matches!(
            parser.next(),
            Some(InputEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::F(6),
                ..
            })))
        ));
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::BackTab, modifiers, .. }))) if modifiers == KeyModifiers::SHIFT)
        );
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::Char('a'), modifiers, .. }))) if modifiers == KeyModifiers::ALT | KeyModifiers::CONTROL)
        );
        assert!(
            matches!(parser.next(), Some(InputEvent::Terminal(Event::Key(KeyEvent { code: KeyCode::F(1), modifiers, .. }))) if modifiers == KeyModifiers::CONTROL)
        );
    }
}
