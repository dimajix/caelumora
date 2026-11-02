//! What a channel holds while it runs (`ChannelState`): its history and from where it holds it, and
//! what of it changed since it was stored (`ChannelStateChange`, issue 193).

use super::ChannelId;
use crate::chain::Position;
use crate::history::{History, WINDOW};
use crate::store::Journaled;
use serde::{Deserialize, Serialize};

/// What a channel holds while it runs: its history, and from where it holds
/// it. Its document - what `Journaled` writes - is the small part alone; the
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
}

impl ChannelState {
    pub fn new(channel_id: ChannelId) -> Self {
        Self { channel_id, history: History::default(), history_from: Position::START }
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
}

impl Journaled for ChannelState {
    type Change = ChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        vec![ChannelStateChange::HistoryFrom(self.history_from)]
    }

    /// The history is marked stored by the store that wrote it.
    fn mark_stored(&mut self) {}

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            ChannelStateChange::HistoryFrom(from) => self.history_from = from,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::testing::{id, make_message};
    use crate::store::testing::{replayed, stored};

    #[test]
    fn a_channel_keeps_the_last_entries_of_its_history_once_they_are_stored() {
        let mut state = ChannelState::new(ChannelId::new());
        state.history.extend((1..=250).map(|n| make_message(id(n))).collect());
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

    #[test]
    fn a_channels_state_journals_its_small_part_and_leaves_its_history_to_the_store() {
        let mut state = ChannelState::new(ChannelId::new());
        state.history.extend(vec![make_message(id(1))]);
        let mut copy = stored(&mut state);
        assert!(copy.history.is_empty(), "the history is not in the document");
        assert!(
            state.history.changes().is_some(),
            "and is marked stored by the store that writes it"
        );

        state.history_from = Position::new(1);
        assert_eq!(state.changes().len(), 1, "the small part, whole");
        replayed(&state, &mut copy);
        assert_eq!((copy.channel_id, copy.history_from.get()), (state.channel_id, 1));
    }
}
