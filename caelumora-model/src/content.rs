//! References to binary content kept in a storage of content (issue 282): what is known of the
//! content, how it is classified, and where its bytes lie. A reference is part of what holds it - a
//! message, whose attachments are what was said with its text (`Message::attachments`), or a tool's
//! result, which gave or showed it to a model (`ToolCallResult::media`, `shown`) -: taken back with
//! it, covered by its digest, and read by whoever may read it. The digest beside it is what the
//! bytes are checked against, never a key, so that what holds it records what it was wherever its
//! bytes lie.

use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::value::text::FileName;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::str;

define_id!(ContentId);

/// A reference to content in a storage of content: an attachment where a message holds it, what a
/// tool gave or showed its model where a tool's result does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentRef {
    /// Its name everywhere: in every copy of what holds it, for its download and in the key of its
    /// bytes.
    pub id: ContentId,
    /// The name of the file, as whoever handed it over gave it.
    pub name: FileName,
    /// What it is, as its bytes say - never as a client claims.
    pub media_type: String,
    /// How many bytes it has.
    pub size: u64,
    /// The digest of its bytes (`digest::content`).
    pub digest: Digest,
    /// Its own (issue 282): a label decides where what was read of it may flow, so that a turn that
    /// was never given confidential content stays below it. Never above the channel it was
    /// said in.
    pub classification: Classification,
    /// The storage of content its bytes lie in, by the name the configuration gives it, and their
    /// key there.
    pub storage: String,
    pub key: String,
    /// The number the transcript it stands in gives it, on a transcript's copy alone: how its
    /// model names it (`reference`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// Its bytes, on the copy of a message a model is given it along with
    /// (`ModelProfile::gives_along`) - never stored.
    #[serde(skip)]
    pub content: Option<Bytes>,
}

impl ContentRef {
    /// Its size as a person reads one: in bytes, kB, MB or GB, by powers of ten, with one decimal
    /// above bytes - `2.3 MB`.
    pub fn size_shown(&self) -> String {
        const UNITS: [&str; 3] = ["kB", "MB", "GB"];
        if self.size < 1000 {
            return format!("{} B", self.size);
        }
        #[expect(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
        let mut size = self.size as f64 / 1000.0;
        let mut unit = 0;
        while size >= 1000.0 && unit + 1 < UNITS.len() {
            size /= 1000.0;
            unit += 1;
        }
        format!("{size:.1} {}", UNITS[unit])
    }

    /// How a model names it, `a3`, if its transcript numbered it.
    pub fn reference(&self) -> Option<String> {
        self.number.map(|number| format!("a{number}"))
    }

    /// The number a model's `reference` names (`a3`: 3), if it names one as `reference` writes it.
    pub fn number_named(reference: &str) -> Option<u32> {
        reference.strip_prefix('a')?.parse().ok().filter(|number| *number > 0)
    }

    /// Whether it is an image every model API takes and a browser shows without running anything:
    /// png, jpeg or webp (issue 282) - given along to a model that sees, downloaded inline.
    pub fn is_image(&self) -> bool {
        matches!(self.media_type.as_str(), PNG | JPEG | WEBP)
    }

    /// Whether it is plain text, as its bytes say (`media_type`): what a model reads a page at a
    /// time (issue 282).
    pub fn is_text(&self) -> bool {
        self.media_type == TEXT
    }
}

const PNG: &str = "image/png";
const JPEG: &str = "image/jpeg";
const WEBP: &str = "image/webp";
const TEXT: &str = "text/plain";

/// What content is, as its first bytes say (issue 282) - never as whoever hands it over claims:
/// png, jpeg, webp, gif, pdf or zip (the Office formats among it) by their signatures, plain text
/// where `head` is UTF-8 without control characters other than whitespace, and
/// `application/octet-stream` for anything else. `head` is what came first, a few hundred bytes;
/// a character cut off at its end does not count against the text.
pub fn media_type(head: &[u8]) -> &'static str {
    match head {
        [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', ..] => PNG,
        [0xff, 0xd8, 0xff, ..] => JPEG,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => WEBP,
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => "image/gif",
        [b'%', b'P', b'D', b'F', b'-', ..] => "application/pdf",
        [b'P', b'K', 3, 4, ..] => "application/zip",
        _ if is_text(head) => TEXT,
        _ => "application/octet-stream",
    }
}

/// Whether `head` reads as text: UTF-8, but for a character cut off at its end, without a control
/// character other than whitespace.
fn is_text(head: &[u8]) -> bool {
    let text = match str::from_utf8(head) {
        Ok(text) => text,
        Err(e) if e.error_len().is_none() => match str::from_utf8(&head[..e.valid_up_to()]) {
            Ok(text) => text,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    !text.chars().any(|c| c.is_control() && !c.is_whitespace())
}

/// References for the tests of this crate and of others.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;
    use crate::digest;

    /// A reference to `name` of the bytes `bytes`, as a channel's storage "kept" keeps them,
    /// classified as a channel is by default.
    #[expect(clippy::expect_used, reason = "test code: a name that is none fails the test that gives it")]
    pub fn make_content_ref(name: &str, bytes: &[u8]) -> ContentRef {
        let id = ContentId::new();
        ContentRef {
            id,
            name: FileName::new(name).expect("a test's name of a file is one"),
            media_type: "image/png".to_string(),
            size: bytes.len() as u64,
            digest: digest::content(bytes),
            classification: Classification::CHANNEL_DEFAULT,
            storage: "kept".to_string(),
            key: format!("attachments/{id}"),
            number: None,
            content: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testing::make_content_ref;

    #[test]
    fn what_content_is_is_told_by_its_first_bytes() {
        assert_eq!(media_type(b"\x89PNG\r\n\x1a\n...."), "image/png");
        assert_eq!(media_type(b"\xff\xd8\xff\xe0..JFIF"), "image/jpeg");
        assert_eq!(media_type(b"RIFF\x10\x00\x00\x00WEBPVP8 "), "image/webp");
        assert_eq!(media_type(b"GIF89a.."), "image/gif");
        assert_eq!(media_type(b"%PDF-1.7\n"), "application/pdf");
        assert_eq!(media_type(b"PK\x03\x04 xlsx"), "application/zip");
        assert_eq!(media_type("a,b\n1,2\nZoë".as_bytes()), "text/plain");
        assert_eq!(media_type(&"Zoë".as_bytes()[..3]), "text/plain", "a character cut off at the end of the head");
        assert_eq!(media_type(b"\x00\x01binary"), "application/octet-stream");
        assert_eq!(media_type(b"<svg onload=..."), "text/plain", "what claims to be an image is text, never an image");
        let image = |media_type: &str| ContentRef { media_type: media_type.to_string(), ..make_content_ref("x", b"x") };
        assert!(image("image/png").is_image() && image("image/webp").is_image() && !image("image/gif").is_image() && !image("text/plain").is_image());
    }
}
