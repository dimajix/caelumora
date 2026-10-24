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
use serde::{Deserialize, Serialize};

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
}

/// References for the tests of this crate.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::digest;

    /// A reference to `name` of the bytes `bytes`, as a channel's storage "kept" keeps them,
    /// classified as a channel is by default.
    pub(crate) fn make_content_ref(name: &str, bytes: &[u8]) -> ContentRef {
        let id = ContentId::new();
        ContentRef {
            id,
            name: FileName::new(name).unwrap(),
            media_type: "image/png".to_string(),
            size: bytes.len() as u64,
            digest: digest::content(bytes),
            classification: Classification::CHANNEL_DEFAULT,
            storage: "kept".to_string(),
            key: format!("attachments/{id}"),
        }
    }
}
