use std::borrow::Cow;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use optionfiles_core::{Clipboard, ClipboardMode, clipboard_decode, clipboard_encode};

const MAX_URI_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_URI_BYTES.div_ceil(3) * 4;
const MAX_PAYLOAD: usize = 4096;
const QUERY_ID: i32 = 731;

#[derive(Debug, PartialEq, Eq)]
pub enum DndEvent {
    Supported,
    OfferGesture { x: u16, y: u16 },
    Hover { x: i32, y: i32 },
    Dropped(Vec<PathBuf>),
    Started,
    Completed,
    Cancelled,
    Error(String),
}

#[derive(Default)]
pub struct Dnd {
    supported: bool,
    querying: bool,
    source_armed: bool,
    gesture: bool,
    hover_uri: bool,
    drop_mime: Option<i32>,
    drop_ready: bool,
    drop_data: Vec<u8>,
    chunk_metadata: Option<String>,
    chunk_payload: String,
    offer_data: Vec<u8>,
    offer_pending: bool,
    offer_started: bool,
    offer_dropped: bool,
    offer_cancelled: bool,
}

fn send(out: &mut impl Write, metadata: &str, payload: &str) -> io::Result<()> {
    write!(out, "\x1b]72;{metadata};{payload}\x1b\\")?;
    out.flush()
}

fn number(metadata: &str, key: &str) -> Option<i32> {
    metadata.split(':').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then(|| value.parse().ok()).flatten()
    })
}

fn kind(metadata: &str) -> Option<&str> {
    metadata.split(':').find_map(|part| part.strip_prefix("t="))
}

fn send_data(out: &mut impl Write, metadata: &str, bytes: &[u8]) -> io::Result<()> {
    let mut encoded = [0; MAX_PAYLOAD];
    for chunk in bytes.chunks(MAX_PAYLOAD / 4 * 3) {
        let count = STANDARD
            .encode_slice(chunk, &mut encoded)
            .expect("base64 output buffer fits");
        send(
            out,
            &format!("{metadata}:m=1"),
            std::str::from_utf8(&encoded[..count]).expect("base64 is ASCII"),
        )?;
    }
    send(out, &format!("{metadata}:m=0"), "")
}

fn local_uris(bytes: &[u8]) -> Result<Vec<PathBuf>> {
    let text = std::str::from_utf8(bytes)?;
    if text
        .bytes()
        .any(|byte| byte.is_ascii_control() && byte != b'\r' && byte != b'\n')
    {
        bail!("URI list contains unescaped control characters");
    }
    Ok(clipboard_decode(bytes, "text/uri-list")?.paths)
}

impl Dnd {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.querying = true;
        send(out, &format!("t=q:i={QUERY_ID}"), "")?;
        write!(out, "\x1b[c")?;
        out.flush()
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    /// Returns true if DA1 closed the query without a protocol response.
    pub fn primary_attributes(&mut self) -> bool {
        let unavailable = self.querying && !self.supported;
        self.querying = false;
        unavailable
    }

    pub fn arm_source(&mut self, armed: bool, out: &mut impl Write) -> Result<()> {
        if !self.supported {
            bail!("terminal native drag protocol is unavailable")
        }
        send(out, if armed { "t=o:x=1" } else { "t=o:x=2" }, "")?;
        self.source_armed = armed;
        self.gesture = false;
        Ok(())
    }

    pub fn source_armed(&self) -> bool {
        self.source_armed
    }

    /// Receives an OSC body, including `72;`, with its framing already removed.
    pub fn handle(&mut self, frame: &str, out: &mut impl Write) -> Result<Option<DndEvent>> {
        let Some(body) = frame.strip_prefix("72;") else {
            return Ok(None);
        };
        let (metadata, payload) = body.split_once(';').unwrap_or((body, ""));
        if payload.len() > MAX_PAYLOAD || metadata.len() > 1024 {
            self.reset_chunks();
            if self.drop_mime.is_some() {
                self.reject_drop(out)?;
            }
            bail!("native drag frame exceeds protocol limits");
        }
        // Queries are permitted between chunks and must not become part of their data.
        if matches!(kind(metadata), Some("q" | "Q")) {
            if kind(metadata) == Some("q")
                && self.querying
                && number(metadata, "i") == Some(QUERY_ID)
            {
                send(out, "t=a", "text/uri-list")?;
                self.querying = false;
                self.supported = true;
                return Ok(Some(DndEvent::Supported));
            }
            return Ok(None);
        }
        if !self.supported {
            return Ok(None);
        }
        let marker = metadata.split(':').find_map(|part| part.strip_prefix("m="));
        let more = match marker.map(str::parse::<u32>).transpose() {
            Ok(None | Some(0)) => false,
            Ok(Some(1)) => true,
            _ => {
                self.reset_chunks();
                if self.drop_mime.is_some() {
                    self.reject_drop(out)?;
                }
                bail!("invalid native drag chunk marker");
            }
        };
        let data_end = payload.is_empty() && !more;
        let (metadata, payload) = if let Some(first) = self.chunk_metadata.take() {
            if self.chunk_payload.len() + payload.len() > MAX_ENCODED_BYTES {
                self.reset_chunks();
                if self.drop_mime.is_some() {
                    self.reject_drop(out)?;
                }
                bail!("native URI payload is too large");
            }
            self.chunk_payload.push_str(payload);
            if more {
                self.chunk_metadata = Some(first);
                return Ok(None);
            }
            (
                Cow::Owned(first),
                Cow::Owned(std::mem::take(&mut self.chunk_payload)),
            )
        } else if more {
            self.chunk_metadata = Some(metadata.to_owned());
            self.chunk_payload.push_str(payload);
            return Ok(None);
        } else {
            (Cow::Borrowed(metadata), Cow::Borrowed(payload))
        };
        self.message(&metadata, &payload, data_end, out)
    }

    fn message(
        &mut self,
        metadata: &str,
        payload: &str,
        data_end: bool,
        out: &mut impl Write,
    ) -> Result<Option<DndEvent>> {
        match kind(metadata) {
            Some("o") if self.source_armed && !self.offer_pending => {
                let (x, y) = (
                    number(metadata, "x").unwrap_or(-1),
                    number(metadata, "y").unwrap_or(-1),
                );
                if let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) {
                    self.gesture = true;
                    Ok(Some(DndEvent::OfferGesture { x, y }))
                } else {
                    Ok(None)
                }
            }
            Some("m") => {
                let x = number(metadata, "x").unwrap_or(-1);
                let y = number(metadata, "y").unwrap_or(-1);
                if x < 0 || y < 0 {
                    self.hover_uri = false;
                } else if !payload.is_empty() {
                    self.hover_uri = payload
                        .split_ascii_whitespace()
                        .any(|mime| mime == "text/uri-list");
                }
                let allowed = number(metadata, "o").unwrap_or(0) & 1 != 0;
                send(
                    out,
                    if x >= 0 && y >= 0 && allowed && self.hover_uri {
                        "t=m:o=1"
                    } else {
                        "t=m:o=0"
                    },
                    "text/uri-list",
                )?;
                Ok(Some(DndEvent::Hover { x, y }))
            }
            Some("M") => {
                if self.drop_mime.is_some() {
                    self.reject_drop(out)?;
                    return Ok(Some(DndEvent::Error(
                        "another native drop is still active".into(),
                    )));
                }
                self.drop_data.clear();
                self.drop_ready = false;
                self.drop_mime = payload
                    .split_ascii_whitespace()
                    .position(|mime| mime == "text/uri-list")
                    .map(|index| index as i32 + 1);
                if number(metadata, "o").unwrap_or(0) & 1 == 0 || self.drop_mime.is_none() {
                    self.reject_drop(out)?;
                    return Ok(Some(DndEvent::Error(
                        "drop does not offer local files with copy permission".into(),
                    )));
                }
                send(out, &format!("t=r:x={}", self.drop_mime.unwrap()), "")?;
                Ok(None)
            }
            Some("r")
                if number(metadata, "x") == self.drop_mime
                    && self.drop_mime.is_some()
                    && !self.drop_ready =>
            {
                if number(metadata, "X").unwrap_or(0) != 0
                    || number(metadata, "y").is_some()
                    || number(metadata, "Y").is_some()
                {
                    self.reject_drop(out)?;
                    bail!("remote file data is not supported");
                }
                if !payload.is_empty() {
                    let decoded = STANDARD
                        .decode(payload)
                        .or_else(|_| STANDARD_NO_PAD.decode(payload));
                    let bytes = match decoded {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            self.reject_drop(out)?;
                            return Err(error.into());
                        }
                    };
                    if self.drop_data.len() + bytes.len() > MAX_URI_BYTES {
                        self.reject_drop(out)?;
                        bail!("native URI payload is too large");
                    }
                    self.drop_data.extend_from_slice(&bytes);
                }
                if !data_end {
                    return Ok(None);
                }
                let paths = match local_uris(&self.drop_data) {
                    Ok(paths) => paths,
                    Err(error) => {
                        self.reject_drop(out)?;
                        return Err(error);
                    }
                };
                self.drop_data.clear();
                self.drop_ready = true;
                Ok(Some(DndEvent::Dropped(paths)))
            }
            Some("R") if self.drop_mime.is_some() => {
                self.drop_mime = None;
                self.drop_ready = false;
                self.drop_data.clear();
                Ok(Some(DndEvent::Error(payload.to_owned())))
            }
            Some("E") if self.offer_pending && payload == "OK" && !self.offer_started => {
                self.offer_started = true;
                if self.offer_cancelled {
                    send(out, "t=E:y=-1", "")?;
                    return Ok(None);
                }
                Ok(Some(DndEvent::Started))
            }
            Some("E") if self.offer_pending && payload != "OK" => {
                self.reset_offer();
                Ok(Some(DndEvent::Error(payload.to_owned())))
            }
            Some("e") if self.offer_started => match number(metadata, "x") {
                Some(2)
                    if number(metadata, "o")
                        .is_some_and(|operation| operation != 0 && operation != 1) =>
                {
                    self.cancel_offer(out)?;
                    Ok(Some(DndEvent::Error(
                        "destination requested a non-copy action".into(),
                    )))
                }
                Some(3) => {
                    self.offer_dropped = true;
                    Ok(None)
                }
                Some(4) => {
                    let completed = self.offer_dropped
                        && !self.offer_cancelled
                        && number(metadata, "y") == Some(0);
                    self.reset_offer();
                    Ok(Some(if completed {
                        DndEvent::Completed
                    } else {
                        DndEvent::Cancelled
                    }))
                }
                Some(5) if number(metadata, "y") == Some(0) => {
                    send_data(out, "t=e:y=0", &self.offer_data)?;
                    Ok(None)
                }
                Some(5) => {
                    send(
                        out,
                        &format!("t=E:y={}", number(metadata, "y").unwrap_or(-1)),
                        "ENOENT",
                    )?;
                    Ok(None)
                }
                _ => Ok(None),
            },
            Some("k") if self.offer_started => {
                send(out, "t=E", "EPERM:local files only")?;
                self.reset_offer();
                Ok(Some(DndEvent::Error("remote drag is not supported".into())))
            }
            _ => Ok(None),
        }
    }

    pub fn begin_offer(&mut self, paths: Vec<PathBuf>, out: &mut impl Write) -> Result<()> {
        if !self.supported {
            bail!("terminal native drag protocol is unavailable")
        }
        if !self.source_armed || !std::mem::take(&mut self.gesture) {
            bail!("native drag requires an armed pointer gesture")
        }
        if self.offer_pending {
            bail!("a native drag is already active")
        }
        if paths.is_empty() {
            bail!("nothing selected to drag")
        }
        for path in &paths {
            path.symlink_metadata()?;
        }
        let data = clipboard_encode(
            &Clipboard {
                paths,
                mode: ClipboardMode::Copy,
            },
            "text/uri-list",
        );
        if data.len() > MAX_URI_BYTES {
            bail!("native URI payload is too large")
        }
        self.offer_data = data;
        self.offer_pending = true;
        self.offer_started = false;
        self.offer_dropped = false;
        self.offer_cancelled = false;
        send(out, "t=o:o=1", "text/uri-list")?;
        send_data(out, "t=p:x=0", &self.offer_data)?;
        send(out, "t=P:x=-1", "")?;
        Ok(())
    }

    /// Call only after all local copies succeeded, while the drop is still active.
    pub fn accept_drop(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.drop_ready {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no fully received native drop to acknowledge",
            ));
        }
        send(out, "t=r:o=1", "")?;
        self.drop_mime = None;
        self.drop_ready = false;
        self.drop_data.clear();
        Ok(())
    }

    pub fn reject_drop(&mut self, out: &mut impl Write) -> io::Result<()> {
        send(out, "t=r:o=0", "")?;
        self.drop_mime = None;
        self.drop_ready = false;
        self.drop_data.clear();
        self.reset_chunks();
        Ok(())
    }

    pub fn cancel_offer(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.gesture = false;
        if self.offer_pending {
            self.offer_cancelled = true;
        }
        if self.offer_started {
            send(out, "t=E:y=-1", "")?;
        }
        Ok(())
    }

    fn reset_chunks(&mut self) {
        self.chunk_metadata = None;
        self.chunk_payload.clear();
    }

    fn reset_offer(&mut self) {
        self.offer_data.clear();
        self.offer_pending = false;
        self.offer_started = false;
        self.offer_dropped = false;
        self.offer_cancelled = false;
    }

    pub fn stop(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.supported {
            if self.drop_mime.is_some() {
                self.reject_drop(out)?;
            }
            self.cancel_offer(out)?;
            send(out, "t=A", "")?;
            send(out, "t=o:x=2", "")?;
        }
        self.source_armed = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected() -> (Dnd, Vec<u8>) {
        let mut dnd = Dnd::new();
        let mut out = Vec::new();
        dnd.start(&mut out).unwrap();
        assert_eq!(
            dnd.handle("72;t=q:i=731;", &mut out).unwrap(),
            Some(DndEvent::Supported)
        );
        (dnd, out)
    }

    fn offered() -> (Dnd, Vec<u8>) {
        let (mut dnd, mut out) = connected();
        dnd.arm_source(true, &mut out).unwrap();
        dnd.handle("72;t=o:x=2:y=3;", &mut out).unwrap();
        dnd.begin_offer(
            vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")],
            &mut out,
        )
        .unwrap();
        (dnd, out)
    }

    #[test]
    fn capability_requires_correlated_response_before_da() {
        let mut dnd = Dnd::new();
        let mut out = Vec::new();
        dnd.start(&mut out).unwrap();
        assert_eq!(dnd.handle("72;t=q:i=99;", &mut out).unwrap(), None);
        assert!(dnd.primary_attributes());
        assert_eq!(dnd.handle("72;t=q:i=731;", &mut out).unwrap(), None);
        assert!(!dnd.supported());
        let (mut dnd, out) = connected();
        assert!(!dnd.primary_attributes());
        assert!(!dnd.source_armed());
        assert!(!String::from_utf8_lossy(&out).contains("t=o:x=1"));
    }

    #[test]
    fn source_requires_explicit_arm_and_gesture() {
        let (mut dnd, mut out) = connected();
        assert_eq!(dnd.handle("72;t=o:x=0:y=0;", &mut out).unwrap(), None);
        assert!(dnd.begin_offer(vec![PathBuf::from("/")], &mut out).is_err());
        dnd.arm_source(true, &mut out).unwrap();
        assert_eq!(
            dnd.handle("72;t=o:x=0:y=0;", &mut out).unwrap(),
            Some(DndEvent::OfferGesture { x: 0, y: 0 })
        );
        dnd.begin_offer(vec![PathBuf::from("/"), PathBuf::from("/tmp")], &mut out)
            .unwrap();
        assert!(String::from_utf8_lossy(&out).contains("t=o:o=1;text/uri-list"));
        assert!(String::from_utf8_lossy(&dnd.offer_data).contains("file:///tmp"));
    }

    #[test]
    fn completion_requires_started_drop_and_non_cancelled_finish() {
        let (mut dnd, mut out) = offered();
        assert_eq!(dnd.handle("72;t=e:x=4:y=0;", &mut out).unwrap(), None);
        assert_eq!(
            dnd.handle("72;t=E;OK", &mut out).unwrap(),
            Some(DndEvent::Started)
        );
        dnd.handle("72;t=e:x=3;", &mut out).unwrap();
        assert_eq!(
            dnd.handle("72;t=e:x=4:y=0;", &mut out).unwrap(),
            Some(DndEvent::Completed)
        );
        assert_eq!(dnd.handle("72;t=e:x=4:y=0;", &mut out).unwrap(), None);
        let (mut dnd, mut out) = offered();
        dnd.handle("72;t=E;OK", &mut out).unwrap();
        dnd.handle("72;t=e:x=3;", &mut out).unwrap();
        dnd.cancel_offer(&mut out).unwrap();
        assert_eq!(
            dnd.handle("72;t=e:x=4:y=0;", &mut out).unwrap(),
            Some(DndEvent::Cancelled)
        );
        let (mut dnd, mut out) = offered();
        dnd.handle("72;t=E;OK", &mut out).unwrap();
        assert_eq!(
            dnd.handle("72;t=e:x=4:y=0;", &mut out).unwrap(),
            Some(DndEvent::Cancelled)
        );
    }

    #[test]
    fn refusal_is_error_not_started_or_completed() {
        let (mut dnd, mut out) = offered();
        assert_eq!(
            dnd.handle("72;t=E;EPERM", &mut out).unwrap(),
            Some(DndEvent::Error("EPERM".into()))
        );
        assert_eq!(dnd.handle("72;t=E;OK", &mut out).unwrap(), None);
        assert_eq!(dnd.handle("72;t=e:x=4:y=0;", &mut out).unwrap(), None);
    }

    #[test]
    fn multiple_uri_drop_preserves_spaces_unicode_and_delays_ack() {
        let (mut dnd, mut out) = connected();
        assert!(dnd.accept_drop(&mut out).is_err());
        dnd.handle("72;t=M:o=1;text/uri-list text/plain", &mut out)
            .unwrap();
        let encoded =
            STANDARD_NO_PAD.encode(b"file:///tmp/two%20words\r\nfile:///tmp/caf%C3%A9\r\n");
        dnd.handle(&format!("72;t=r:x=1:m=1;{}", &encoded[..11]), &mut out)
            .unwrap();
        dnd.handle(&format!("72;m=0;{}", &encoded[11..]), &mut out)
            .unwrap();
        assert_eq!(
            dnd.handle("72;t=r:x=1;", &mut out).unwrap(),
            Some(DndEvent::Dropped(vec![
                PathBuf::from("/tmp/two words"),
                PathBuf::from("/tmp/café")
            ]))
        );
        assert!(!String::from_utf8_lossy(&out).contains("t=r:o=1"));
        dnd.accept_drop(&mut out).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("t=r:o=1"));
        assert!(dnd.accept_drop(&mut out).is_err());
    }

    #[test]
    fn query_between_chunks_does_not_corrupt_data() {
        let (mut dnd, mut out) = connected();
        dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
        let encoded = STANDARD.encode(b"file:///tmp/a\r\n");
        dnd.handle(&format!("72;t=r:x=1:m=1;{}", &encoded[..5]), &mut out)
            .unwrap();
        dnd.handle("72;t=q:i=999;future=1", &mut out).unwrap();
        dnd.handle(&format!("72;m=1;{}", &encoded[5..]), &mut out)
            .unwrap();
        assert_eq!(
            dnd.handle("72;m=0;", &mut out).unwrap(),
            Some(DndEvent::Dropped(vec![PathBuf::from("/tmp/a")]))
        );
    }

    #[test]
    fn unsafe_or_invalid_uri_data_rejects_drop() {
        for uri in [
            "file://otherhost/tmp/a",
            "file:///tmp/%00x",
            "file:///tmp/x?query",
            "file:///tmp/%GG",
            "file:///tmp/x\0",
            "file:///tmp/\tbad",
            "https://example.com/a",
        ] {
            let (mut dnd, mut out) = connected();
            dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
            dnd.handle(&format!("72;t=r:x=1;{}", STANDARD.encode(uri)), &mut out)
                .unwrap();
            assert!(dnd.handle("72;t=r:x=1;", &mut out).is_err(), "{uri:?}");
            assert!(String::from_utf8_lossy(&out).contains("t=r:o=0"));
            assert!(!String::from_utf8_lossy(&out).contains("t=r:o=1"));
        }
    }

    #[test]
    fn invalid_base64_and_oversized_frames_release_drop() {
        let (mut dnd, mut out) = connected();
        dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
        assert!(dnd.handle("72;t=r:x=1;!!!", &mut out).is_err());
        assert!(dnd.drop_mime.is_none());
        dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
        assert!(
            dnd.handle(
                &format!("72;t=r:x=1:m=1;{}", "a".repeat(MAX_PAYLOAD + 1)),
                &mut out
            )
            .is_err()
        );
        assert!(dnd.chunk_metadata.is_none());
        assert!(dnd.drop_mime.is_none());
    }

    #[test]
    fn outbound_base64_is_bounded_and_roundtrips() {
        let bytes = vec![0xa5; 17003];
        let mut out = Vec::new();
        send_data(&mut out, "t=p:x=0", &bytes).unwrap();
        let text = String::from_utf8(out).unwrap();
        let mut encoded = String::new();
        for frame in text.split("\x1b]72;").filter(|part| !part.is_empty()) {
            let (_, payload) = frame
                .strip_suffix("\x1b\\")
                .unwrap()
                .split_once(';')
                .unwrap();
            assert!(payload.len() <= MAX_PAYLOAD);
            encoded.push_str(payload);
        }
        assert!(text.ends_with("t=p:x=0:m=0;\x1b\\"));
        assert_eq!(STANDARD.decode(encoded).unwrap(), bytes);
    }

    #[test]
    fn hover_remembers_mime_until_leave() {
        let (mut dnd, mut out) = connected();
        dnd.handle("72;t=m:x=1:y=1:o=1;text/plain", &mut out)
            .unwrap();
        out.clear();
        dnd.handle("72;t=m:x=2:y=1:o=1;", &mut out).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("t=m:o=0"));
        dnd.handle("72;t=m:x=2:y=1:o=1;text/uri-list", &mut out)
            .unwrap();
        out.clear();
        dnd.handle("72;t=m:x=3:y=1:o=1;", &mut out).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("t=m:o=1"));
        dnd.handle("72;t=m:x=-1:y=-1:o=0;", &mut out).unwrap();
        assert!(!dnd.hover_uri);
    }

    #[test]
    fn malformed_chunk_markers_cancel_active_drop() {
        for marker in ["2", "-1", "invalid", "4294967296"] {
            let (mut dnd, mut out) = connected();
            dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
            assert!(
                dnd.handle(&format!("72;t=r:x=1:m={marker};"), &mut out)
                    .is_err()
            );
            assert!(dnd.drop_mime.is_none());
            assert!(String::from_utf8_lossy(&out).contains("t=r:o=0"));
        }
    }

    #[test]
    fn only_first_chunk_metadata_is_authoritative() {
        let (mut dnd, mut out) = connected();
        dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
        let encoded = STANDARD.encode(b"file:///tmp/a\r\n");
        dnd.handle(&format!("72;t=r:x=1:m=1;{}", &encoded[..7]), &mut out)
            .unwrap();
        dnd.handle(&format!("72;t=r:x=999:m=1;{}", &encoded[7..]), &mut out)
            .unwrap();
        assert_eq!(
            dnd.handle("72;m=0;", &mut out).unwrap(),
            Some(DndEvent::Dropped(vec![PathBuf::from("/tmp/a")]))
        );
    }

    #[test]
    fn aggregate_encoded_and_decoded_limits_release_drop() {
        let (mut dnd, mut out) = connected();
        dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
        dnd.chunk_metadata = Some("t=r:x=1:m=1".into());
        dnd.chunk_payload = "A".repeat(MAX_ENCODED_BYTES);
        assert!(dnd.handle("72;m=1;AAAA", &mut out).is_err());
        assert!(dnd.drop_mime.is_none());
        assert!(dnd.chunk_metadata.is_none());
        assert!(dnd.chunk_payload.is_empty());
        dnd.handle("72;t=M:o=1;text/uri-list", &mut out).unwrap();
        dnd.drop_data = vec![b'a'; MAX_URI_BYTES];
        assert!(dnd.handle("72;t=r:x=1;YQ==", &mut out).is_err());
        assert!(dnd.drop_mime.is_none());
        assert!(dnd.drop_data.is_empty());
    }

    #[test]
    fn cancellation_before_start_is_deferred_until_terminal_ack() {
        let (mut dnd, mut out) = offered();
        out.clear();
        dnd.cancel_offer(&mut out).unwrap();
        assert!(out.is_empty());
        assert_eq!(dnd.handle("72;t=E;OK", &mut out).unwrap(), None);
        assert!(String::from_utf8_lossy(&out).contains("t=E:y=-1"));
        dnd.handle("72;t=e:x=3;", &mut out).unwrap();
        assert_eq!(
            dnd.handle("72;t=e:x=4:y=0;", &mut out).unwrap(),
            Some(DndEvent::Cancelled)
        );
    }

    #[test]
    fn move_only_drop_is_rejected_without_requesting_data() {
        let (mut dnd, mut out) = connected();
        out.clear();
        assert!(matches!(
            dnd.handle("72;t=M:o=2;text/uri-list", &mut out).unwrap(),
            Some(DndEvent::Error(_))
        ));
        assert!(String::from_utf8_lossy(&out).contains("t=r:o=0"));
        assert!(!String::from_utf8_lossy(&out).contains("t=r:x="));
    }

    #[test]
    fn finish_after_user_cancel_is_never_completion() {
        let (mut dnd, mut out) = offered();
        dnd.handle("72;t=E;OK", &mut out).unwrap();
        dnd.handle("72;t=e:x=3;", &mut out).unwrap();
        assert_eq!(
            dnd.handle("72;t=e:x=4:y=1;", &mut out).unwrap(),
            Some(DndEvent::Cancelled)
        );
    }
}
