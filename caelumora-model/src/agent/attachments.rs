//! The numbers an agent's transcript gives the attachments it holds (issue 282): how its model
//! names them.

use crate::content::ContentId;
use crate::message::Message;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The numbers an agent's transcript gives the attachments it holds, each once (issue 282): how its
/// model names them (`a3`, `ContentRef::reference`), so that each agent of a channel has numbers of
/// its own. Kept in the agent's state rather than found in its transcript, which it holds a window
/// of alone and a compaction begins anew, while a number has to resolve beyond both. A number is
/// never given again: one removed from a message is no longer found, not given to another.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttachmentIndex {
    numbers: BTreeMap<ContentId, u32>,
    /// The numbers given since the state was stored, as a change records them.
    #[serde(skip)]
    unstored: Vec<(ContentId, u32)>,
}

impl AttachmentIndex {
    /// Whether it gave no number yet.
    pub fn is_empty(&self) -> bool {
        self.numbers.is_empty()
    }

    /// Numbers the attachments of `message`, the transcript's copy: each the number it was given
    /// before - one a modification keeps -, else the next.
    pub(crate) fn number(&mut self, message: &mut Message) {
        for attachment in &mut message.attachments {
            attachment.number = Some(self.number_of(attachment.id));
        }
    }

    fn number_of(&mut self, id: ContentId) -> u32 {
        if let Some(number) = self.numbers.get(&id) {
            return *number;
        }
        let number = u32::try_from(self.numbers.len()).map_or(u32::MAX, |given| given.saturating_add(1));
        self.numbers.insert(id, number);
        self.unstored.push((id, number));
        number
    }

    /// The numbers given since the state was stored.
    pub(crate) fn changes(&self) -> &[(ContentId, u32)] {
        &self.unstored
    }

    pub(crate) fn mark_stored(&mut self) {
        self.unstored.clear();
    }

    /// Takes in the numbers `given`, as a change recorded them.
    pub(crate) fn apply(&mut self, given: Vec<(ContentId, u32)>) {
        self.numbers.extend(given);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Classification;
    use crate::content::testing::make_content_ref;

    #[test]
    fn an_attachment_is_numbered_once_and_keeps_its_number() {
        let (a, b) = (make_content_ref("a.png", b"a"), make_content_ref("b.png", b"b"));
        let mut index = AttachmentIndex::default();
        let mut first = Message { attachments: vec![a.clone()], ..Message::new(Classification::CHANNEL_DEFAULT) };
        index.number(&mut first);
        let mut modified = Message { attachments: vec![b, a], ..first.clone() };
        index.number(&mut modified);
        let numbers = |message: &Message| message.attachments.iter().map(|a| a.reference()).collect::<Vec<_>>();
        assert_eq!(numbers(&first), [Some("a1".to_string())]);
        assert_eq!(numbers(&modified), [Some("a2".to_string()), Some("a1".to_string())], "kept by a modification, the new one the next");
        assert_eq!(index.changes().len(), 2, "each recorded once");

        let stored: AttachmentIndex = serde_json::from_value(serde_json::to_value(&index).unwrap()).unwrap();
        let mut replayed = AttachmentIndex::default();
        replayed.apply(index.changes().to_vec());
        for mut again in [stored, replayed] {
            let mut later = Message { attachments: vec![make_content_ref("c.png", b"c")], ..first.clone() };
            again.number(&mut later);
            assert_eq!(numbers(&later), [Some("a3".to_string())], "never given again, stored or replayed");
        }
    }
}
