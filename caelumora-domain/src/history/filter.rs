//! What a page of a history is filtered by (issues 421, 451): an item as it is kept to be judged
//! (`KeptItem`) - the item as it is recorded, its position, whether it is published and the thread
//! it happened in -, the fields of it a filter names (`HistoryField`), and the conditions over them
//! (`Filter<HistoryField>`): what a reader may be given of each entry (`Filter::seen_by`, as
//! `Recorded::as_seen_by` decides it, issue 417), the messages of one sender, those heard, those of
//! a classification the reader may copy - conditions as data, so that a database answers them as a
//! `WHERE`. The histories a store keeps, and how it reads them, are `caelumora_store::history`'s.

use super::{Attended, Reader, Recorded};
use crate::actor::ActorId;
use crate::chain::Position;
use crate::channel::ThreadId;
use crate::confidentiality::{Classification, Compartments, Confidentiality, Reading};
use crate::message::MessageRole;
use crate::query::{Field, Filter};
use serde::{Deserialize, Serialize};

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
    /// The calls the message carries (issue 587); absent where it carries none.
    ToolCalls,
    /// What a call says it does (`ToolCall::description`), inside `Filter::Any` over `ToolCalls`.
    CallDescription,
    Mentions,
    Level,
    Compartments,
    /// An element of a collection itself, inside `Filter::Any`.
    Element,
    /// Whose envelope the item was (`Recorded::sender`), the system for
    /// what the channel says itself - not always the message's sender.
    Origin,
    /// Whether it went to every user, every agent, the channel's approvers
    /// that took part at the time (`Recipients`, issue 417).
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
    /// The thread it happened in (`Recorded::thread`, issue 557); absent for what happened in the
    /// whole channel.
    Thread,
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
            HistoryField::ToolCalls => &["entry", "said", "message", "tool_calls"],
            HistoryField::CallDescription => &["description"],
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
            HistoryField::Thread => &["thread"],
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

    /// A message that says something: words, or attachments (issue 282) -
    /// `Message::says_something`.
    pub fn saying() -> Self {
        Filter::any_of([Filter::written(), Filter::with_attachments()])
    }

    /// A message carrying attachments (issues 282, 509) - not what its tool results gave their
    /// model, which is seen, not said.
    pub fn with_attachments() -> Self {
        Filter::negated(Filter::IsNull(HistoryField::Attachments))
    }

    /// A message containing `words` (`Filter::Contains`).
    pub fn containing(words: &str) -> Self {
        Filter::contains(HistoryField::Content, words)
    }

    /// A message the other participants hear (`Message::is_heard`): words
    /// or attachments of a person or an agent, or the calls a person made
    /// (issue 587), not a step of a plan under way.
    pub fn heard() -> Self {
        Filter::all_of([
            Filter::is_in(HistoryField::Role, [MessageRole::User, MessageRole::Assistant]),
            Filter::eq(HistoryField::Internal, false),
            Filter::any_of([Filter::saying(), Filter::called_by_a_person()]),
        ])
    }

    /// A message carrying calls a person made themselves (issue 587): calls in the role of a user -
    /// `Message::is_a_persons_call`, which `Message::is_heard` counts.
    pub fn called_by_a_person() -> Self {
        Filter::all_of([
            Filter::eq(HistoryField::Role, MessageRole::User),
            Filter::negated(Filter::IsNull(HistoryField::ToolCalls)),
        ])
    }

    /// A message carrying a call whose description contains `words` (`Filter::Contains`, issue
    /// 587): what a call says it does, in words for a person.
    pub fn with_a_call_containing(words: &str) -> Self {
        Filter::any(HistoryField::ToolCalls, Filter::contains(HistoryField::CallDescription, words))
    }

    /// What happened in `thread` or in the whole channel (issue 557): a page of one thread -
    /// `Recorded::thread` being `None` or `thread`.
    pub fn in_thread(thread: ThreadId) -> Self {
        Filter::any_of([
            Filter::IsNull(HistoryField::Thread),
            Filter::eq(HistoryField::Thread, thread),
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

    /// What `reader`, with `attended` as where it took part (`Attendance::of_reader`), is given of
    /// a history (issues 451, 557): the rule of `Recorded::seen_by` as a condition over an item as
    /// it is kept (`KeptItem`), which a database adds to the `WHERE` of a page; no condition for
    /// the channel, which is given everything. `seen_by` decides the same in memory.
    pub fn seen_by(reader: &Reader, attended: &[Attended]) -> Option<Self> {
        let published = Filter::eq(HistoryField::Published, true);
        match reader {
            Reader::Channel => None,
            Reader::Outsider => {
                Some(Filter::all_of([published, Filter::eq(HistoryField::ToUsers, true)]))
            }
            Reader::Participant(actor) => {
                // What went to every participant of `kind`, and `where_it_took_part` of the
                // reader's: a user while it took part, an agent while and where - in that thread,
                // or in the whole channel (`Attended::holds`).
                let while_taking_part = |kind, where_it_took_part: &dyn Fn(&Attended) -> Self| {
                    let within = attended.iter().map(where_it_took_part).collect();
                    Filter::all_of([Filter::eq(kind, true), Filter::Or(within)])
                };
                let positions = |at: &Attended| {
                    let range = at.range.start.get()..at.range.end.get();
                    Filter::within(HistoryField::Position, range)
                };
                let in_its_thread =
                    |at: &Attended| Filter::all_of([positions(at), Filter::in_thread(at.thread)]);
                let of_kind = match actor {
                    ActorId::User(_) => while_taking_part(HistoryField::ToUsers, &positions),
                    ActorId::Agent(_) => while_taking_part(HistoryField::ToAgents, &in_its_thread),
                    ActorId::Approver(_) => Filter::eq(HistoryField::ToApprovers, true),
                    // What goes to the users and happened in its thread or the whole channel
                    // (`Recipients::reaches`).
                    ActorId::Delegate(thread) => Filter::all_of([
                        Filter::eq(HistoryField::ToUsers, true),
                        Filter::in_thread(*thread),
                    ]),
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

/// An item as it is kept to be judged (issue 451): the item as it is recorded, with its position,
/// whether it is published and the thread it happened in (issue 557) - derived as it is made, as
/// the item is from the chain -, so that a filter of a page names them (`HistoryField`). What a
/// database keeps of an item, and what a store in memory judges a filter on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeptItem {
    pub seq: Position,
    pub published: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    #[serde(flatten)]
    pub item: Recorded,
}

impl KeptItem {
    /// `item`, at the position `seq`, as it is kept.
    pub fn of(seq: Position, item: Recorded) -> Self {
        Self { seq, published: item.entry.is_published(), thread: item.thread(), item }
    }
}
