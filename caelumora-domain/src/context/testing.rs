//! What the tests of the context's parts share: a message saying a text.

use crate::confidentiality::Classification;
use crate::message::{Message, MessageId};

pub(super) fn text(content: &str) -> Message {
    Message {
        id: MessageId::new(),
        content: Some(content.to_string()),
        ..Message::new(Classification::CHANNEL_DEFAULT)
    }
}
