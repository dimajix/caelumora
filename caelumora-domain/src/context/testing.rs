//! What the tests of the context's parts share: a message saying a text.

use crate::message::testing::make_message;
use crate::message::{Message, MessageId};

/// A message saying `content`, as `make_message` makes one.
pub(super) fn text(content: &str) -> Message {
    Message { content: Some(content.to_string()), ..make_message(MessageId::new()) }
}
