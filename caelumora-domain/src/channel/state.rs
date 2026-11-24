//! What a channel holds while it runs (`ChannelState`): its history and from where it holds it, its
//! threads, and what of it changed since it was stored (`ChannelStateChange`, issue 193).

use super::{ChannelId, Delegation, ThreadId, ThreadState};
use crate::actor::ActorId;
use crate::chain::Position;
use crate::history::{History, HistoryEntry, WINDOW};
use crate::store::Journaled;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a channel holds while it runs: its history, from where it holds
/// it, and its threads. Its document - what `Journaled` writes - is the small part alone; the
/// history is a chain a store keeps apart (issue 379), written with the state
/// and read on its own (`HistoryStore`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_id: ChannelId,
    /// The channel's history, from `history_from` on: as a store loads it.
    #[serde(skip)]
    pub history: History,
    /// The position from which on the channel keeps its history in memory;
    /// a store loads the history from there.
    pub history_from: Position,
    /// The threads of the channel, its line among them, those closed too (issue 557): what it
    /// builds the open ones from as it starts, and what tells a thread closed from one it never
    /// had. Written by the channel as it records their opening and closing.
    pub threads: BTreeMap<ThreadId, ThreadState>,
}

impl ChannelState {
    /// The state of a channel that has said nothing yet: its line the one thread open.
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            history: History::default(),
            history_from: Position::START,
            threads: BTreeMap::from([(ThreadId::line(channel_id), ThreadState::LINE)]),
        }
    }

    /// Follows what a thread's delegate gave, as `entry` records it at `at` (issue 557,
    /// `Delegation::follow`); the thread whose delegation it moved on, if it moved one on.
    pub fn follow_given(&mut self, entry: &HistoryEntry, at: Position) -> Option<ThreadId> {
        let HistoryEntry::GivenToRead { recipient, message, by: ActorId::Delegate(thread), .. } =
            entry
        else {
            return None;
        };
        self.delegation(*thread)?.follow(*recipient, message, at).then_some(*thread)
    }

    /// Follows the channel refusing what a thread's delegate gave, as `entry` would have recorded
    /// it (issue 557, `Delegation::refused`); the thread whose delegation it moved on, if it moved
    /// one on.
    pub fn follow_refused(&mut self, entry: &HistoryEntry) -> Option<ThreadId> {
        let HistoryEntry::GivenToRead { recipient, message, by: ActorId::Delegate(thread), .. } =
            entry
        else {
            return None;
        };
        self.delegation(*thread)?.refused(*recipient, message).then_some(*thread)
    }

    /// Follows `reader` reading what came of a brief of theirs, as `entry` records it (issue 598,
    /// `Delegation::follow_read`); the thread whose delegation it moved on.
    pub fn follow_read(&mut self, reader: ActorId, entry: &HistoryEntry) -> Option<ThreadId> {
        let HistoryEntry::Given { message } = entry else {
            return None;
        };
        self.threads.iter_mut().find_map(|(thread, held)| {
            let delegation = held.delegation.as_mut()?;
            delegation.follow_read(reader, message.id).then_some(*thread)
        })
    }

    /// Follows what lets a giver's delegations go, as `entry` records it in `thread`, after what it
    /// recorded before (issue 598), in the thread they were opened at: its turn ending there after
    /// it read what came of the last brief, with no brief given since
    /// (`Delegation::follow_turn_end`); its leave of that thread, alone or with the channel - where
    /// nothing comes back to it any more (`Delegation::follow_leave`). A delegation into a thread
    /// that does not own its recipient is never let go: nothing would end it. The threads let go.
    pub fn follow_giver(&mut self, entry: &HistoryEntry, thread: ThreadId) -> Vec<ThreadId> {
        let owning = self.threads.iter_mut().filter(|(_, held)| held.owns_its_recipient());
        let mut let_go = Vec::new();
        for (id, held) in owning {
            let opened_at = held.opening.as_ref().map(|opening| opening.origin.thread);
            let Some(delegation) = held.delegation.as_mut().filter(|held| !held.is_let_go()) else {
                continue;
            };
            let goes = match entry {
                HistoryEntry::TurnFinished { actor, .. } if opened_at == Some(thread) => {
                    delegation.follow_turn_end(*actor)
                }
                HistoryEntry::ParticipantLeft { participant, .. } if opened_at == Some(thread) => {
                    delegation.follow_leave(*participant)
                }
                _ => false,
            };
            if goes {
                let_go.push(*id);
            }
        }
        let_go
    }

    /// The threads not closed whose delegation was let go (issue 598): what is still to be ended.
    pub fn delegations_let_go(&self) -> Vec<ThreadId> {
        let let_go =
            |held: &ThreadState| held.delegation.as_ref().is_some_and(Delegation::is_let_go);
        let open = self.threads.iter().filter(|(_, held)| !held.closed && let_go(held));
        open.map(|(thread, _)| *thread).collect()
    }

    /// The delegation of `thread`, if it has one.
    pub fn delegation(&mut self, thread: ThreadId) -> Option<&mut Delegation> {
        self.threads.get_mut(&thread).and_then(|held| held.delegation.as_mut())
    }

    /// Lets go of what its history holds before its window - the last
    /// `WINDOW` entries - once it is stored, and records where the window
    /// begins, to be stored with the next put (issue 379). The channel reads
    /// no further back than that in memory; everything else is the store's.
    pub fn keep_window(&mut self) {
        self.history.keep_from(self.history.end().before(WINDOW));
        self.history_from = self.history.base();
    }
}

/// A change to a [`ChannelState`] since it was last stored (issue 193). The
/// channel's id is what the state is stored under, not a part that changes;
/// its history is a chain a store keeps apart (issue 379).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStateChange {
    /// From where the history is held, whole.
    HistoryFrom(Position),
    /// The threads, whole (issue 557).
    Threads(BTreeMap<ThreadId, ThreadState>),
}

impl Journaled for ChannelState {
    type Change = ChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        vec![
            ChannelStateChange::HistoryFrom(self.history_from),
            ChannelStateChange::Threads(self.threads.clone()),
        ]
    }

    /// The history is marked stored by the store that wrote it.
    fn mark_stored(&mut self) {}

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            ChannelStateChange::HistoryFrom(from) => self.history_from = from,
            ChannelStateChange::Threads(threads) => self.threads = threads,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::context_in;
    use crate::message::testing::{id, make_message};
    use crate::store::testing::{replayed, stored};

    #[test]
    fn a_channel_keeps_the_last_entries_of_its_history_once_they_are_stored() {
        let channel = ChannelId::new();
        let mut state = ChannelState::new(channel);
        state
            .history
            .extend(&context_in(channel), (1..=250).map(|n| make_message(id(n))).collect());
        state.keep_window();
        assert_eq!(
            (state.history.base().get(), state.history_from.get()),
            (0, 0),
            "nothing goes before it is stored"
        );
        state.history.mark_stored();
        state.keep_window();
        assert_eq!(
            (state.history.base().get(), state.history.end().get(), state.history_from.get()),
            (50, 250, 50),
            "the last entries"
        );
    }

    /// Issue 598: a channel follows a giver reading what came back, and lets go the delegations
    /// into threads that own their recipient as the giver's turn ends where they were opened, or as
    /// it leaves the thread there; one into a thread that does not own its recipient stays, and so
    /// does one whose giver's turn ends in another thread, or is given up.
    #[test]
    fn a_channel_lets_go_what_its_giver_left_without_a_word() {
        use super::super::{Brief, Circle, Opening, ThreadOrigin, Word};
        use crate::agent::AgentId;
        use crate::confidentiality::Classification;
        use crate::message::{Message, MessageId};
        use crate::turn::GivenUpReason;
        use crate::work::TurnId;
        use chrono::Utc;
        use std::collections::VecDeque;

        let channel = ChannelId::new();
        let line = ThreadId::line(channel);
        let mut state = ChannelState::new(channel);
        let giver = ActorId::Agent(AgentId::new());
        let at = Position::new;
        let mut answers = Vec::new();
        let mut threads = Vec::new();
        for owns in [true, false] {
            let recipient = AgentId::new();
            let answer = Message {
                id: MessageId::new(),
                sender_id: ActorId::Agent(recipient),
                ..Message::new(Classification::CHANNEL_DEFAULT)
            };
            let brief =
                Brief { message: Message::new(Classification::CHANNEL_DEFAULT), given_at: None };
            let mut delegation = Delegation {
                giver,
                recipient: ActorId::Agent(recipient),
                initiator: giver,
                briefs: VecDeque::from([brief]),
                word: Word::NotDue,
            };
            delegation.follow(giver, &answer, at(2));
            let origin = ThreadOrigin { thread: line, message: MessageId::new() };
            let owned = if owns { vec![recipient] } else { Vec::new() };
            let opening = Opening { origin, by: giver, circle: Circle::Closed, owned };
            let thread = ThreadId::new();
            let held =
                ThreadState { opening: Some(opening), closed: false, delegation: Some(delegation) };
            state.threads.insert(thread, held);
            answers.push(answer);
            threads.push(thread);
        }
        for answer in &answers {
            let given = HistoryEntry::Given { message: answer.clone() };
            assert!(state.follow_read(giver, &given).is_some(), "read by its giver");
        }
        let finished =
            HistoryEntry::TurnFinished { turn_id: TurnId::new(), actor: giver, at: Utc::now() };

        assert!(state.follow_giver(&finished, ThreadId::new()).is_empty(), "elsewhere");
        let reason = GivenUpReason::Retried;
        let given_up = HistoryEntry::TurnGivenUp {
            turn_id: TurnId::new(),
            actor: giver,
            reason,
            at: Utc::now(),
        };
        assert!(state.follow_giver(&given_up, line).is_empty(), "given up: its word due");
        assert_eq!(state.follow_giver(&finished, line), [threads[0]], "the owning one");
        assert_eq!(state.delegations_let_go(), [threads[0]], "to be ended");
        let other = state.delegation(threads[1]).expect("delegated");
        assert!(!other.is_let_go(), "one not owning its recipient stays");

        let left = |out_of_channel| HistoryEntry::ParticipantLeft {
            participant: giver,
            name: "giver".to_string(),
            by: giver,
            by_name: "giver".to_string(),
            until: at(6),
            at: Utc::now(),
            out_of_channel,
        };
        if let Some(held) = state.threads.get_mut(&threads[0]) {
            held.delegation.as_mut().expect("delegated").word = Word::NotDue;
        }
        let elsewhere = ThreadId::new();
        assert!(state.follow_giver(&left(true), elsewhere).is_empty(), "another thread left");
        assert_eq!(state.follow_giver(&left(false), line), [threads[0]], "where it was opened");
    }

    /// Issue 557: a channel's line is open from its start.
    #[test]
    fn a_new_channel_has_its_line_open() {
        let channel = ChannelId::new();
        let state = ChannelState::new(channel);
        let line = ThreadId::line(channel);
        assert_eq!(state.threads.keys().collect::<Vec<_>>(), [&line], "its line, alone");
        assert!(
            matches!(
                state.threads[&line],
                ThreadState { opening: None, closed: false, delegation: None }
            ),
            "open, hanging nowhere"
        );
    }

    #[test]
    fn a_channels_state_journals_its_small_part_and_leaves_its_history_to_the_store() {
        let channel = ChannelId::new();
        let mut state = ChannelState::new(channel);
        state.history.extend(&context_in(channel), vec![make_message(id(1))]);
        let mut copy = stored(&mut state);
        assert!(copy.history.is_empty(), "the history is not in the document");
        assert!(
            state.history.changes().is_some(),
            "and is marked stored by the store that writes it"
        );

        state.history_from = Position::new(1);
        state.threads.insert(ThreadId::new(), ThreadState::LINE);
        assert_eq!(state.changes().len(), 2, "the small part, whole");
        replayed(&state, &mut copy);
        assert_eq!((copy.channel_id, copy.history_from.get()), (state.channel_id, 1));
        let held = |state: &ChannelState| {
            let threads = state.threads.iter();
            threads.map(|(id, held)| (*id, held.opening.clone(), held.closed)).collect::<Vec<_>>()
        };
        assert_eq!(held(&copy), held(&state), "its threads with it");
    }
}
