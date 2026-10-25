//! The numbers an agent's transcript gives the attachments it holds (issue 282): how its model
//! names them.

use crate::content::{ContentId, ContentRef};
use crate::message::{Message, MessageId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The numbers an agent's transcript gives the attachments it holds, each once (issue 282): how its
/// model names them (`a3`, `ContentRef::reference`), so that each agent of a channel has numbers of
/// its own - each with the message that carries the attachment, through which a tool reads it as
/// it stands. Kept in the agent's state rather than found in its transcript, which it holds a
/// window of alone and a compaction begins anew, while a number has to resolve beyond both. A
/// number is never given again: one removed from a message is no longer found there, not given to
/// another.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "Vec<(MessageId, ContentId)>", into = "Vec<(MessageId, ContentId)>")]
pub struct AttachmentIndex {
    /// Each attachment numbered, with the message that carries it, in the order numbered: its
    /// number is its position, counted from one.
    numbered: Vec<(MessageId, ContentId)>,
    /// The number of each, by its id.
    numbers: HashMap<ContentId, u32>,
    /// How many of `numbered` were stored.
    stored: usize,
}

impl From<Vec<(MessageId, ContentId)>> for AttachmentIndex {
    /// The index as it was stored.
    fn from(numbered: Vec<(MessageId, ContentId)>) -> Self {
        let mut index = Self::default();
        index.apply(numbered);
        index
    }
}

impl From<AttachmentIndex> for Vec<(MessageId, ContentId)> {
    fn from(index: AttachmentIndex) -> Self {
        index.numbered
    }
}

impl AttachmentIndex {
    /// Whether it gave no number yet.
    pub fn is_empty(&self) -> bool {
        self.numbered.is_empty()
    }

    /// The attachment a model names `reference` (`a3`), and the message that carries it, if this
    /// transcript numbered one so.
    pub fn named(&self, reference: &str) -> Option<(MessageId, ContentId)> {
        let number = usize::try_from(ContentRef::number_named(reference)?).ok()?;
        self.numbered.get(number - 1).copied()
    }

    /// Numbers the attachments of `message`, the transcript's copy, and what its tool results
    /// showed: each the number it was given before - one a modification keeps, one shown again -,
    /// else the next, with `message` as what carries it.
    pub(crate) fn number(&mut self, message: &mut Message) {
        let carrier = message.id;
        for attachment in message.carried_mut() {
            attachment.number = Some(self.give(carrier, attachment.id));
        }
    }

    /// The number of `attachment`, carried by `carrier`: the one it was given, else the next.
    fn give(&mut self, carrier: MessageId, attachment: ContentId) -> u32 {
        if let Some(number) = self.numbers.get(&attachment) {
            return *number;
        }
        self.numbered.push((carrier, attachment));
        let number = u32::try_from(self.numbered.len()).unwrap_or(u32::MAX);
        self.numbers.insert(attachment, number);
        number
    }

    /// The attachments numbered since the state was stored, in order.
    pub(crate) fn changes(&self) -> &[(MessageId, ContentId)] {
        &self.numbered[self.stored..]
    }

    pub(crate) fn mark_stored(&mut self) {
        self.stored = self.numbered.len();
    }

    /// Takes in the attachments `numbered`, in order, as a stored change recorded them - each
    /// once.
    pub(crate) fn apply(&mut self, numbered: Vec<(MessageId, ContentId)>) {
        for (carrier, attachment) in numbered {
            self.give(carrier, attachment);
        }
        self.mark_stored();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Classification;
    use crate::content::testing::make_content_ref;

    #[test]
    fn an_attachment_is_numbered_once_keeps_its_number_and_names_its_message() {
        let (a, b) = (make_content_ref("a.png", b"a"), make_content_ref("b.png", b"b"));
        let mut index = AttachmentIndex::default();
        let mut first = Message { attachments: vec![a.clone()], ..Message::new(Classification::CHANNEL_DEFAULT) };
        index.number(&mut first);
        let mut modified = Message { attachments: vec![b.clone(), a.clone()], ..first.clone() };
        index.number(&mut modified);
        let numbers = |message: &Message| message.attachments.iter().map(|a| a.reference()).collect::<Vec<_>>();
        assert_eq!(numbers(&first), [Some("a1".to_string())]);
        assert_eq!(numbers(&modified), [Some("a2".to_string()), Some("a1".to_string())], "kept by a modification, the new one the next");
        assert_eq!(index.named("a2"), Some((first.id, b.id)), "with the message that carries it");
        for unknown in ["a3", "a0", "3", "ax", ""] {
            assert_eq!(index.named(unknown), None, "{unknown}");
        }
        assert_eq!(index.changes().len(), 2, "each recorded once");

        let stored: AttachmentIndex = serde_json::from_value(serde_json::to_value(&index).unwrap()).unwrap();
        assert!(stored.changes().is_empty(), "what was read was stored");
        let mut replayed = AttachmentIndex::default();
        replayed.apply(index.changes().to_vec());
        replayed.apply(index.changes().to_vec());
        assert!(replayed.changes().is_empty(), "replayed is stored");
        for mut again in [stored, replayed] {
            let mut later = Message { attachments: vec![make_content_ref("c.png", b"c")], ..first.clone() };
            again.number(&mut later);
            assert_eq!(numbers(&later), [Some("a3".to_string())], "never given again, stored or replayed - twice");
        }
    }
}
