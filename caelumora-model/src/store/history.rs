//! A history as a store keeps it (issue 379): its chain, which is the truth
//! and only appended to, and beside it the items as they stand - every
//! message with every change since folded into it - and who was a member
//! when (`Memberships`), both derived from the chain and rebuilt from it at
//! any time. The chain is written by the put of the state that holds it
//! (`StateStore`); the `HistoryStore` is the read side, for whoever reads a
//! history without holding it - always as a reader, who is given what of
//! each entry they may see (`Recorded::as_seen_by`, issue 417): a predicate
//! over what the store keeps of an entry and the reader's ranges, which the
//! model gives as data too (`Filter::seen_by`, issue 451), so that a
//! database answers it as a `WHERE`. What else a page keeps - the messages
//! of one sender, those heard, those of a classification the reader may
//! copy - is a condition as data as well (`Filter<HistoryField>`, issue
//! 421). Both are judged on an item as it is kept (`KeptItem`).

use crate::actor::ActorId;
use crate::chain::Position;
use crate::confidentiality::{Classification, Compartments, Confidentiality, Reading};
use crate::history::Reader;
use crate::message::MessageRole;
use crate::store::query::{Field, Filter};
use std::fmt::Debug;
use std::ops::Range;

/// What a page of a history may be filtered by (`HistoryStore::page`,
/// issue 421): where it lies in an item as it is kept (`KeptItem`) - the
/// item as it is recorded (`Recorded`), its position and whether it is
/// published. The fields of a message are there only for a message said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryField {
    /// The message said; absent for every other entry.
    Message,
    Sender,
    Role,
    Internal,
    Content,
    /// What the message carries besides its words (issue 282); absent
    /// where it carries nothing.
    Attachments,
    Mentions,
    Level,
    Compartments,
    /// An element of a collection itself, inside `Filter::Any`.
    Element,
    /// Whose envelope the item was (`Recorded::sender`), the system for
    /// what the channel says itself - not always the message's sender.
    Origin,
    /// Whether it went to every user, every agent, the channel's approvers
    /// that were members at the time (`Recipients`, issue 417).
    ToUsers,
    ToAgents,
    ToApprovers,
    /// Whom it went to by name.
    Named,
    /// Where it lies in the chain (issue 451).
    Position,
    /// Whether anything of it is for others than its sender
    /// (`HistoryEntry::is_published`).
    Published,
}

impl Field for HistoryField {
    fn path(self) -> &'static [&'static str] {
        match self {
            HistoryField::Message => &["entry", "said", "message"],
            HistoryField::Sender => &["entry", "said", "message", "sender_id"],
            HistoryField::Role => &["entry", "said", "message", "role"],
            HistoryField::Internal => &["entry", "said", "message", "internal"],
            HistoryField::Content => &["entry", "said", "message", "content"],
            HistoryField::Attachments => &["entry", "said", "message", "attachments"],
            HistoryField::Mentions => &["entry", "said", "message", "mentions"],
            HistoryField::Level => &["entry", "said", "message", "classification", "level"],
            HistoryField::Compartments => {
                &["entry", "said", "message", "classification", "compartments"]
            }
            HistoryField::Element => &[],
            HistoryField::Origin => &["sender"],
            HistoryField::ToUsers => &["recipients", "users"],
            HistoryField::ToAgents => &["recipients", "agents"],
            HistoryField::ToApprovers => &["recipients", "approvers"],
            HistoryField::Named => &["recipients", "named"],
            HistoryField::Position => &["seq"],
            HistoryField::Published => &["published"],
        }
    }
}

/// The conditions a page of a history is read by.
impl Filter<HistoryField> {
    /// A message said.
    pub fn said() -> Self {
        Filter::negated(Filter::IsNull(HistoryField::Message))
    }

    /// A message `sender` said.
    pub fn said_by(sender: ActorId) -> Self {
        Filter::all_of([Filter::said(), Filter::eq(HistoryField::Sender, sender)])
    }

    /// A message with words in it.
    pub fn written() -> Self {
        Filter::all_of([
            Filter::negated(Filter::IsNull(HistoryField::Content)),
            Filter::negated(Filter::eq(HistoryField::Content, "")),
        ])
    }

    /// A message that says something: words, or attachments (issue 282).
    pub fn saying() -> Self {
        Filter::any_of([
            Filter::written(),
            Filter::negated(Filter::IsNull(HistoryField::Attachments)),
        ])
    }

    /// A message containing `words` (`Filter::Contains`).
    pub fn containing(words: &str) -> Self {
        Filter::contains(HistoryField::Content, words)
    }

    /// A message the other participants hear (`Message::is_heard`): words
    /// or attachments of a person or an agent, not a step of a plan under
    /// way.
    pub fn heard() -> Self {
        Filter::all_of([
            Filter::is_in(HistoryField::Role, [MessageRole::User, MessageRole::Assistant]),
            Filter::eq(HistoryField::Internal, false),
            Filter::saying(),
        ])
    }

    /// A message mentioning `actor`.
    pub fn mentioning(actor: ActorId) -> Self {
        Filter::any(HistoryField::Mentions, Filter::eq(HistoryField::Element, actor))
    }

    /// What `reading` lets a reader read of a channel, with the configured
    /// `compartments`: no condition for the whole, else the messages whose
    /// classification it admits (`Reading::admits`) - as a condition over
    /// their level and compartments.
    pub fn readable(reading: &Reading, compartments: &Compartments) -> Option<Self> {
        match reading {
            Reading::Whole => None,
            Reading::Nothing => Some(Filter::nothing()),
            Reading::Released(into) => {
                Some(Filter::all_of([Filter::said(), Filter::flowing_into(into, compartments)]))
            }
        }
    }

    /// What `reader`, with `ranges` as theirs (`Memberships::of_reader`),
    /// is given of a history (issue 451): the rule of `Recorded::seen_by`
    /// as a condition over an item as it is kept (`KeptItem`), which a
    /// database adds to the `WHERE` of a page; no condition for the channel,
    /// which is given everything. `seen_by` decides the same in memory.
    pub fn seen_by(reader: &Reader, ranges: &[Range<Position>]) -> Option<Self> {
        let published = Filter::eq(HistoryField::Published, true);
        match reader {
            Reader::Channel => None,
            Reader::Outsider => {
                Some(Filter::all_of([published, Filter::eq(HistoryField::ToUsers, true)]))
            }
            Reader::Participant(actor) => {
                let while_a_member = |kind| {
                    let within = ranges
                        .iter()
                        .map(|range| {
                            Filter::within(
                                HistoryField::Position,
                                range.start.get()..range.end.get(),
                            )
                        })
                        .collect();
                    Filter::all_of([Filter::eq(kind, true), Filter::Or(within)])
                };
                let of_kind = match actor {
                    ActorId::User(_) => while_a_member(HistoryField::ToUsers),
                    ActorId::Agent(_) => while_a_member(HistoryField::ToAgents),
                    ActorId::Approver(_) => Filter::eq(HistoryField::ToApprovers, true),
                    ActorId::System | ActorId::None => Filter::nothing(),
                };
                let reaching = Filter::Or(vec![
                    Filter::any(HistoryField::Named, Filter::eq(HistoryField::Element, *actor)),
                    of_kind,
                ]);
                Some(Filter::Or(vec![
                    Filter::eq(HistoryField::Origin, *actor),
                    Filter::all_of([published, reaching]),
                ]))
            }
        }
    }

    /// A message classified so that it flows into `into`
    /// (`Classification::flows_to`): of its level or lower, and general, or
    /// meant for a compartment each of the destination's may read.
    fn flowing_into(into: &Classification, compartments: &Compartments) -> Self {
        let levels = Confidentiality::LEVELS.into_iter().filter(|level| *level <= into.level());
        let meant_for_each = match into.compartments().is_empty() {
            true => Filter::nothing(),
            false => Filter::all_of(into.compartments().iter().map(|reader| {
                Filter::any(
                    HistoryField::Compartments,
                    Filter::is_in(HistoryField::Element, compartments.readable_by(reader)),
                )
            })),
        };
        Filter::all_of([
            Filter::is_in(HistoryField::Level, levels),
            Filter::Or(vec![Filter::IsNull(HistoryField::Compartments), meant_for_each]),
        ])
    }
}
