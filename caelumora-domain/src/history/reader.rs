//! Who reads a history and where they stand in it (issues 223, 417): the reader, who is given what
//! of an entry they may see (`Reader`), and the positions delivered to them (`Heard`).

use crate::actor::ActorId;
use crate::chain::Position;
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Every position of a history: the range of a participant from its start on.
pub const EVERY_POSITION: Range<Position> = Position::START..Position::MAX;

/// Where a reader of a history stands in it (issue 223): `next`, the first
/// position not yet delivered to it in order, and `taken`, the positions
/// after it delivered ahead of their turn - an approval taken while a turn
/// waits, the rest kept for later (`EventReceiver::recv_matching`). What a
/// reader that falls behind reads on from, and what an agent stores with
/// what it took in, so that a restart delivers what the stored state lacks,
/// nothing twice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heard {
    next: Position,
    /// Each after `next`, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    taken: Vec<Position>,
}

impl Heard {
    /// Nothing delivered from `next` on.
    pub fn at(next: Position) -> Self {
        Self { next, taken: Vec::new() }
    }

    /// The first position not delivered in order.
    pub fn next(&self) -> Position {
        self.next
    }

    /// Whether what is at `seq` was delivered.
    pub fn has(&self, seq: Position) -> bool {
        seq < self.next || self.taken.binary_search(&seq).is_ok()
    }

    /// What is at `seq` was delivered in order: everything before it was
    /// delivered, or is not for the reader - and so was what was taken
    /// ahead right after it.
    pub fn took(&mut self, seq: Position) {
        self.next = self.next.max(seq.next());
        let next = self.next;
        self.taken.retain(|taken| *taken >= next);
        while self.taken.first() == Some(&self.next) {
            self.taken.remove(0);
            self.next = self.next.next();
        }
    }

    /// What is at `seq` was delivered ahead of its turn - in order, if it
    /// is the next.
    pub fn took_ahead(&mut self, seq: Position) {
        if seq == self.next {
            return self.took(seq);
        }
        if let Err(at) = self.taken.binary_search(&seq) {
            if seq > self.next {
                self.taken.insert(at, seq);
            }
        }
    }
}

/// Who reads a history, and so what of an entry they are given
/// (`Recorded::as_seen_by`, issue 417). There is no read of a history
/// without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    /// A participant, now or formerly - or whom may read what an agent saw,
    /// reading as that agent: what went to them and what they sent, the
    /// latter in full.
    Participant(ActorId),
    /// Someone who may read the channel but takes no part in it - its
    /// session's owner, a reader from another channel: what went to
    /// everybody, as everybody was given it.
    Outsider,
    /// The channel itself, for its own checks - whether a message can be
    /// changed: everything, as it was recorded.
    Channel,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue 223: where a reader stands - what was delivered in order, and
    /// what ahead of it, forgotten once passed.
    #[test]
    fn a_reader_stands_after_what_was_delivered_in_order_and_ahead() {
        let at = Position::new;
        let mut heard = Heard::at(at(3));
        assert!(heard.has(at(2)) && !heard.has(at(3)), "before where it begins");
        heard.took_ahead(at(6));
        heard.took_ahead(at(2));
        assert!(heard.has(at(6)) && !heard.has(at(5)), "taken ahead");
        heard.took(at(4));
        assert_eq!(
            heard.next(),
            at(5),
            "past what was delivered in order, what lies between not for it"
        );
        assert!(heard.has(at(6)), "still taken ahead");
        heard.took(at(5));
        assert_eq!(heard, Heard::at(at(7)), "and what was taken ahead right after it");
        heard.took_ahead(at(9));
        heard.took(at(10));
        assert_eq!(heard, Heard::at(at(11)), "what was taken ahead is forgotten once passed");
        heard.took(at(1));
        assert_eq!(heard.next(), at(11), "never back");
        let stored: Heard = serde_json::from_value(serde_json::to_value(&heard).unwrap()).unwrap();
        assert_eq!(stored, heard);
    }
}
