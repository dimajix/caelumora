//! How far a reader reads what lies at a place (issues 133, 134, 421): the places a copy goes
//! between (`Place`) and the bound of a reading (`Reading`).

use super::{Classification, Compartments};
use crate::channel::{Channel, ChannelId};
use crate::privacy::SessionPrivacy;

/// One side of a copy (`ConfidentialityQuestions::may_copy_information`): a channel, the
/// classification of what is read there or of the channel that reads, and
/// the session it belongs to as privacy sees it (issue 134). Whether two
/// places are one channel, or in one session, is told by their ids.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub channel: ChannelId,
    pub classification: Classification,
    pub session: SessionPrivacy,
}

impl Place {
    /// Where `channel` lies, in a session that privacy sees as `session`, classified as the
    /// channel is: what is read when the channel is read.
    pub fn of(channel: &Channel, session: SessionPrivacy) -> Self {
        Self { channel: channel.id, classification: channel.classification.clone(), session }
    }
}

/// How far a reader reads what lies at a place (issues 133, 421): the whole,
/// what was released to it - each entry classified so that it flows into a
/// classification, the reader's -, or nothing. A reader's relation to a
/// channel: the bound is the reader's, the channel decides whether it holds
/// at all (its session's consent) or is not needed (it flows whole). Asked
/// of a `ConfidentialityPolicy` as what of a channel may be copied into a
/// place, once; data rather than a decision for each entry, so that a store
/// reads only what may be read (`HistoryField`). How far, not who reads -
/// that is `history::Reader`, whom a store gives which entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Whole,
    Released(Classification),
    Nothing,
}

impl Reading {
    /// Whether the reader reads what is classified `classification`, with
    /// the configured `compartments`.
    pub fn admits(&self, classification: &Classification, compartments: &Compartments) -> bool {
        match self {
            Reading::Whole => true,
            Reading::Released(into) => classification.flows_to(into, compartments),
            Reading::Nothing => false,
        }
    }
}
