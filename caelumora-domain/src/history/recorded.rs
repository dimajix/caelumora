//! An entry as it is recorded (issue 417, `Recorded`): whose envelope it was, whom it went to - as
//! a moderator decides it (`Audience`), resolved as it is recorded (`Recipients`) - and the head of
//! the envelope it came in, so that it is delivered again as it came (issue 223); and what a reader
//! is given of it (`Recorded::seen_by`, `Recorded::as_seen_by`).

use super::{HistoryEntry, Reader};
use crate::actor::{ActorId, Envelope, EventContext};
use crate::chain::Position;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::ops::{Not, Range};

/// What happened in a channel, recorded (issue 417): the entry, the actor
/// whose envelope it was - the system for what the channel says itself -,
/// whom it went to, resolved as it was recorded into what a store filters by
/// (`Recipients`), and the head of the envelope it came in (issue 223): its
/// context and when it was sent - what it is delivered again with from the
/// history (`envelope`). The chain's entry of a `History`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recorded {
    pub entry: HistoryEntry,
    pub sender: ActorId,
    pub recipients: Recipients,
    /// The context of the envelope as it entered the channel, without a
    /// position, which is the chain's. Its sender is the envelope's - for a
    /// request to respond, whose context it carries, while `sender` is the
    /// system, in whose name it is recorded.
    pub context: EventContext,
    pub timestamp: DateTime<Utc>,
}

/// What a range of a history - as recorded, or as a reader is given it -
/// folds by (`fold`).
impl AsRef<HistoryEntry> for Recorded {
    fn as_ref(&self) -> &HistoryEntry {
        &self.entry
    }
}

/// Whom something in a channel goes to, as a moderator decides it (issues
/// 417, 418).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    /// Everyone it concerns, by the rule every channel delivers by
    /// (`Envelope::recipients`, read off an entry as `HistoryEntry::recipients`).
    #[default]
    Everybody,
    /// These alone; nobody if empty.
    Only(Vec<ActorId>),
}

/// Whom something in a channel goes to, resolved as it is recorded (issue
/// 417): every participant of the kinds it names at the time - whoever's
/// range holds the entry's position -, the channel's approvers, and whom it
/// names, participant or not. What a store filters by - for a database,
/// columns of the row and a `WHERE` over them and the reader's ranges
/// (`Recorded::seen_by`); and, read off an envelope, whom a channel delivers
/// it to (`Envelope::recipients`). Nobody if it names nobody.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipients {
    /// Every user that took part at the time.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub users: bool,
    /// Every agent that took part at the time.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub agents: bool,
    /// The channel's approvers: its own actors, no participants (issue
    /// 420), and so of no range - there all along.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub approvers: bool,
    /// These by name, participant or not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named: Vec<ActorId>,
}

impl Recipients {
    /// Nobody.
    pub const NOBODY: Recipients =
        Recipients { users: false, agents: false, approvers: false, named: Vec::new() };

    /// Every user that took part at the time, and every agent too if
    /// `agents`.
    pub fn participants(agents: bool) -> Self {
        Self { users: true, agents, ..Self::NOBODY }
    }

    /// The channel's approvers.
    pub const APPROVERS: Recipients =
        Recipients { users: false, agents: false, approvers: true, named: Vec::new() };

    /// `named` alone.
    pub fn only(named: Vec<ActorId>) -> Self {
        Self { named, ..Self::NOBODY }
    }

    /// Every user that took part at the time, or, if `shown_to` names
    /// some, the users among them alone - by name, participant or not.
    pub fn users(shown_to: Option<Vec<ActorId>>) -> Self {
        match shown_to {
            None => Self::participants(false),
            Some(shown_to) => Self::only(
                shown_to.into_iter().filter(|actor| matches!(actor, ActorId::User(_))).collect(),
            ),
        }
    }

    /// Whether it goes to `actor` for what `actor` is or by its name -
    /// whether a user or an agent took part at the time is the ranges'
    /// to say (`reaches`).
    pub fn names(&self, actor: ActorId) -> bool {
        self.named.contains(&actor) || self.of_kind(actor)
    }

    /// Whether it goes to `actor` for what it is: a user or an agent of the
    /// kinds it names, or an approver.
    fn of_kind(&self, actor: ActorId) -> bool {
        match actor {
            ActorId::User(_) => self.users,
            ActorId::Agent(_) => self.agents,
            ActorId::Approver(_) => self.approvers,
            ActorId::System | ActorId::None => false,
        }
    }

    /// Whether it reached `actor` at the position `seq`, `ranges` being
    /// where `actor` took part: by name; as an approver; as a user or an
    /// agent while it took part.
    fn reaches(&self, actor: ActorId, seq: Position, ranges: &[Range<Position>]) -> bool {
        self.named.contains(&actor)
            || (self.approvers && matches!(actor, ActorId::Approver(_)))
            || (self.of_kind(actor) && ranges.iter().any(|range| range.contains(&seq)))
    }
}

impl Recorded {
    /// `entry`, sent by `sender` to `audience` - to everybody resolved into
    /// whom it concerns by the rule every channel delivers by
    /// (`HistoryEntry::recipients`) -, recorded by its holder itself: of no
    /// envelope, its head names the sender alone, now.
    pub fn new(entry: HistoryEntry, sender: ActorId, audience: Audience) -> Self {
        let context =
            EventContext { sender_id: sender, initiator_id: sender, ..EventContext::default() };
        Self::with_head(entry, sender, audience, context, Utc::now())
    }

    /// What a channel records of `envelope` (issue 223): the entry it
    /// records (`Envelope::entry`), sent by `sender` - the system for a
    /// request to respond, else the envelope's sender - to `audience`,
    /// with the envelope's context and time; `None` if nothing of it is
    /// recorded.
    pub fn of(envelope: &Envelope, sender: ActorId, audience: Audience) -> Option<Self> {
        let context = EventContext { position: None, ..envelope.context.clone() };
        Some(Self::with_head(envelope.entry()?, sender, audience, context, envelope.timestamp))
    }

    fn with_head(
        entry: HistoryEntry,
        sender: ActorId,
        audience: Audience,
        context: EventContext,
        timestamp: DateTime<Utc>,
    ) -> Self {
        let recipients = match audience {
            Audience::Everybody => entry.recipients(),
            Audience::Only(only) => Recipients::only(only),
        };
        Self { entry, sender, recipients, context, timestamp }
    }

    /// `entry` as every channel delivers it, sent by whom it names
    /// (`HistoryEntry::named_sender`): what a holder records of what it was
    /// told, or did itself.
    pub(super) fn delivered(entry: HistoryEntry) -> Self {
        let sender = entry.named_sender();
        Self::new(entry, sender, Audience::Everybody)
    }

    /// The envelope it came in, delivered again from the history at the
    /// position `seq` (issue 223): its event (`HistoryEntry::event`), its
    /// context with the position, and its time. `None` for an entry of no
    /// event. What is delivered of it is the channel's to stamp, as it stamps
    /// what it delivers live.
    pub fn envelope(&self, seq: Position) -> Option<Envelope> {
        let event = self.entry.event()?;
        Some(Envelope {
            timestamp: self.timestamp,
            context: EventContext { position: Some(seq), ..self.context.clone() },
            event,
        })
    }

    /// Whether `reader` is given anything of the entry at the position `seq`
    /// (issue 417), `ranges` being the reader's as a participant - where it
    /// was given what went to every participant (`Attendance`): the channel
    /// everything; a participant what it sent, and, if anything of it is
    /// published, what went to it by name or what went to every participant
    /// of its kind while it was one; an outsider what was published to every
    /// participant. The one rule by which a history is read, a predicate over
    /// what a store keeps of the entry - its sender, its recipients, whether
    /// it is published, its position - and the reader's ranges, so that a
    /// database answers it as a `WHERE`: the model gives it as a filter too
    /// (`Filter::seen_by`, issue 451), which decides the same.
    pub fn seen_by(&self, seq: Position, reader: &Reader, ranges: &[Range<Position>]) -> bool {
        match reader {
            Reader::Channel => true,
            Reader::Participant(actor) if *actor == self.sender => true,
            Reader::Participant(actor) => {
                self.entry.is_published() && self.recipients.reaches(*actor, seq, ranges)
            }
            Reader::Outsider => self.entry.is_published() && self.recipients.users,
        }
    }

    /// What `reader` may see of the entry at the position `seq`, with
    /// `ranges` as `seen_by` takes them: nothing unless it sees it; then the
    /// entry as it was recorded for its sender and the channel, and as
    /// everybody was given it for anyone else (`HistoryEntry::published`).
    pub fn as_seen_by(
        &self,
        seq: Position,
        reader: &Reader,
        ranges: &[Range<Position>],
    ) -> Option<HistoryEntry> {
        if !self.seen_by(seq, reader, ranges) {
            return None;
        }
        match reader {
            Reader::Channel => Some(self.entry.clone()),
            Reader::Participant(actor) if *actor == self.sender => Some(self.entry.clone()),
            Reader::Participant(_) | Reader::Outsider => self.entry.published(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorEvent, Rejection, RequestReason, RetryTarget};
    use crate::agent::{AgentId, RetryRefused};
    use crate::approval::{ApprovalDecision, ApprovalRequest, ApprovalVerdict};
    use crate::confidentiality::Classification;
    use crate::context::FrameRef;
    use crate::digest;
    use crate::digest::message_chain;
    use crate::history::testing::items_of;
    use crate::history::{History, EVERY_POSITION};
    use crate::message::testing::{id, make_message};
    use crate::message::{Message, MessageRole};
    use crate::question::{QuestionId, QuestionRequest};
    use crate::turn::GivenUpReason;
    use crate::user::UserId;
    use crate::work::TurnId;
    use uuid::Uuid;

    /// Issue 417: a reader is given an entry only if it went to them or
    /// they sent it - an outsider what went to everybody, the channel
    /// everything -, in full only if they sent it.
    #[test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    fn a_reader_is_given_what_went_to_them_and_what_they_sent_in_full() {
        use crate::tool::{ToolCall, ToolVisibility};
        // Each reader taking part all along.
        const ALL: &[Range<Position>] = &[EVERY_POSITION];
        let agent = ActorId::Agent(AgentId::new());
        let other = ActorId::Agent(AgentId::new());
        let user = ActorId::User(UserId::new());
        let approver = ActorId::Approver(Default::default());
        let hidden = ToolCall {
            id: Some("1".to_string()),
            function_name: "look".to_string(),
            arguments: serde_json::json!({}),
            visibility: ToolVisibility::HIDDEN,
            presentation: Default::default(),
        };
        let answer = Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            content: Some("found it".to_string()),
            tool_calls: vec![hidden],
            turn_id: Some(TurnId::new()),
            ..make_message(id(1))
        }
        .produced_from_context(None, digest::frame(&FrameRef::default()));
        let said =
            Recorded::new(HistoryEntry::Said { message: answer }, agent, Audience::Everybody);
        let calls = |entry: Option<HistoryEntry>| {
            entry
                .as_ref()
                .and_then(HistoryEntry::said)
                .map(|m| (m.tool_calls.len(), m.content.clone()))
        };

        assert_eq!(
            calls(said.as_seen_by(Position::START, &Reader::Participant(agent), ALL)),
            Some((1, Some("found it".to_string()))),
            "its sender, in full"
        );
        assert!(
            said.as_seen_by(Position::START, &Reader::Participant(agent), ALL)
                .and_then(|e| e.context().map(|_| ()))
                .is_some(),
            "with what it was produced from"
        );
        for reader in [Reader::Participant(user), Reader::Participant(other), Reader::Outsider] {
            let seen = said.as_seen_by(Position::START, &reader, ALL);
            assert_eq!(
                calls(seen.clone()),
                Some((0, Some("found it".to_string()))),
                "{reader:?}: as it was published"
            );
            assert!(
                seen.and_then(|e| e.context().map(|_| ())).is_none(),
                "{reader:?}: without what it was produced from"
            );
        }
        assert!(
            said.as_seen_by(Position::START, &Reader::Participant(approver), ALL).is_none(),
            "it concerns no approver"
        );
        assert_eq!(
            calls(said.as_seen_by(Position::START, &Reader::Channel, ALL)),
            Some((1, Some("found it".to_string()))),
            "the channel, as it was recorded"
        );

        let step = Recorded::new(
            HistoryEntry::Said {
                message: Message { sender_id: agent, internal: true, ..make_message(id(2)) },
            },
            agent,
            Audience::Everybody,
        );
        assert!(
            step.as_seen_by(Position::START, &Reader::Participant(user), ALL).is_some()
                && step.as_seen_by(Position::START, &Reader::Participant(other), ALL).is_none(),
            "a step of a plan: the users', no other agent's"
        );
        let ended = Recorded::new(
            HistoryEntry::TurnFinished { turn_id: TurnId::new(), actor: agent, at: Utc::now() },
            agent,
            Audience::Everybody,
        );
        assert!(
            ended.as_seen_by(Position::START, &Reader::Participant(agent), ALL).is_some()
                && ended.as_seen_by(Position::START, &Reader::Participant(other), ALL).is_none(),
            "a turn's end: its own and the users'"
        );

        let to_one = Recorded::new(
            HistoryEntry::Said { message: make_message(id(3)) },
            ActorId::System,
            Audience::Only(vec![user]),
        );
        assert!(
            to_one.as_seen_by(Position::START, &Reader::Participant(user), ALL).is_some(),
            "whom it went to"
        );
        assert!(
            to_one
                .as_seen_by(
                    Position::START,
                    &Reader::Participant(ActorId::User(UserId::new())),
                    ALL
                )
                .is_none(),
            "no other user"
        );
        assert!(
            to_one.as_seen_by(Position::START, &Reader::Outsider, ALL).is_none(),
            "nor an outsider: it did not go to everybody"
        );
        let to_nobody = Recorded::new(
            HistoryEntry::Said { message: Message { sender_id: agent, ..make_message(id(4)) } },
            agent,
            Audience::Only(Vec::new()),
        );
        assert!(
            to_nobody.as_seen_by(Position::START, &Reader::Participant(agent), ALL).is_some()
                && to_nobody.as_seen_by(Position::START, &Reader::Participant(user), ALL).is_none(),
            "to nobody: its sender alone"
        );
    }

    /// Every event the history records, each with its sender - `agent`
    /// unless it is a user's or an approver's -, about `user`.
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    fn recorded_events(agent: ActorId, user: ActorId) -> Vec<(ActorId, ActorEvent)> {
        use crate::approval::ApproverId;
        use crate::question::{Addressee, Question, Questions};
        use crate::tool::ToolCall;
        let turn_id = TurnId::new();
        let heard = Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            content: Some("hello".to_string()),
            ..make_message(id(1))
        };
        let step = Message { internal: true, ..heard.clone() };
        let transcript = message_chain(None, &heard);
        let ActorId::Agent(agent_id) = agent else { panic!("{agent} is no agent") };
        let approver = ActorId::Approver(ApproverId::new());
        let call = ToolCall {
            id: None,
            function_name: "noop".to_string(),
            arguments: serde_json::json!({}),
            visibility: Default::default(),
            presentation: Default::default(),
        };
        let approval = ApprovalRequest {
            suggested: None,
            explicit: false,
            id: Default::default(),
            session_id: Default::default(),
            channel_id: Default::default(),
            agent_id,
            tool_call: call.clone(),
            description: "test".to_string(),
        };
        let response = ApprovalVerdict::new(approval.id, agent_id);
        let questions = |sensitive| QuestionRequest {
            id: QuestionId::new(),
            session_id: Default::default(),
            channel_id: Default::default(),
            agent_id,
            tool_call: call.clone(),
            questions: Questions {
                questions: vec![Question {
                    header: "Name".to_string(),
                    question: "Who?".to_string(),
                    choices: Vec::new(),
                    multi_select: false,
                }],
                ask: vec![Addressee::Actor(user)],
                sensitive,
            },
        };
        let by_agent = [
            ActorEvent::CompleteMessageGenerated(heard.clone()),
            ActorEvent::IntermediateMessageGenerated(step),
            ActorEvent::MessageGiven { message: heard.clone() },
            ActorEvent::ContextCompacted { turn_id, from: transcript, to: transcript },
            ActorEvent::TurnFinished { turn_id, messages: Vec::new() },
            ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Left },
            ActorEvent::PlanStepStarted {
                turn_id,
                step_index: 0,
                total_steps: 1,
                description: "first".to_string(),
            },
            ActorEvent::CommandExecuted {
                command: "compact".to_string(),
                args: String::new(),
                output: "done".to_string(),
                to: None,
                turn_id: Some(turn_id),
            },
            ActorEvent::ActivityFinished {
                turn_id,
                activity: "compacting".to_string(),
                output: "done".to_string(),
            },
            ActorEvent::ParticipantJoined { participant: user, name: "user".to_string() },
            ActorEvent::ParticipantLeft {
                participant: user,
                name: "user".to_string(),
                until: Position::new(3),
            },
            ActorEvent::MessageModified(heard.clone()),
            ActorEvent::MessageRevoked { message: heard.id, at: Utc::now() },
            ActorEvent::MessageReclassified {
                message: heard.id,
                classification: Classification::PUBLIC,
            },
            ActorEvent::CommandExecuted {
                command: "mark".to_string(),
                args: String::new(),
                output: "yours".to_string(),
                to: Some(user),
                turn_id: Some(turn_id),
            },
            ActorEvent::CommandExecuted {
                command: "agent wakeup".to_string(),
                args: String::new(),
                output: "woke".to_string(),
                to: None,
                turn_id: None,
            },
            ActorEvent::CommandFailed {
                command: "mark".to_string(),
                reason: "no".to_string(),
                to: user,
                turn_id: None,
            },
            ActorEvent::RetryAccepted { target: RetryTarget::Turn(turn_id) },
            ActorEvent::RetryRefused {
                target: RetryTarget::Turn(turn_id),
                reason: RetryRefused::NotLast,
            },
            ActorEvent::ChainCutOff {
                message: heard.id,
                max_agent_hops: 3,
                not_asked: vec![agent],
            },
            ActorEvent::ResponderUndecided { message: heard.id },
            ActorEvent::EventRejected {
                rejected: "MessageModified".to_string(),
                message: Some(heard.id),
                reason: Rejection::Invalid,
            },
            ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
            ActorEvent::HistoryShared {
                participant: user,
                from: Position::new(3),
                by: agent,
                by_name: "agent".to_string(),
            },
            ActorEvent::ApprovalRequested(approval.clone()),
            ActorEvent::ApprovalRequestWithdrawn { id: approval.id },
            ActorEvent::QuestionRequested(questions(false)),
            ActorEvent::QuestionRequested(questions(true)),
            ActorEvent::QuestionClosed { id: QuestionId::new(), shown_to: None },
            ActorEvent::QuestionClosed { id: QuestionId::new(), shown_to: Some(vec![user]) },
        ];
        let mut events: Vec<(ActorId, ActorEvent)> =
            by_agent.into_iter().map(|event| (agent, event)).collect();
        events.extend([
            (
                user,
                ActorEvent::CommandRequested {
                    command: "mark".to_string(),
                    args: "it".to_string(),
                    agent,
                },
            ),
            (
                user,
                ActorEvent::RetryRequested { target: RetryTarget::Turn(turn_id), author: agent },
            ),
            (
                user,
                ActorEvent::ApprovalAnswered {
                    id: approval.id,
                    approved: true,
                    decision: ApprovalDecision {
                        by: user,
                        name: "user".to_string(),
                        by_rule: None,
                        at: Default::default(),
                    },
                },
            ),
            (
                approver,
                ActorEvent::EventRefused {
                    to: user,
                    refused: "ApprovalAnswered".to_string(),
                    message: None,
                    reason: Rejection::Forbidden,
                },
            ),
            (approver, ActorEvent::ApprovalRequested(approval.clone())),
            (approver, ActorEvent::ApprovalRequestWithdrawn { id: approval.id }),
            (approver, ActorEvent::ApprovalGranted(response.clone())),
            (approver, ActorEvent::ApprovalDenied(response.clone())),
            (approver, ActorEvent::ApprovalTimedOut(response)),
        ]);
        events
    }

    /// Issues 417, 223: what the history records of an envelope goes to
    /// whom the envelope went to - one rule, read off the event and,
    /// resolved as it is recorded, off the entry.
    #[test]
    fn an_entry_goes_to_whom_its_envelope_went_to() {
        let (agent, user) = (ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()));
        for (sender, event) in recorded_events(agent, user) {
            let kind = event.kind();
            let envelope =
                Envelope::new(EventContext { sender_id: sender, ..EventContext::default() }, event);
            let recorded = Recorded::of(&envelope, ActorId::System, Audience::Everybody)
                .unwrap_or_else(|| panic!("{kind} is recorded"));
            assert!(
                !envelope.event.is_transient(),
                "{kind}: what is recorded is delivered from the history, never lost"
            );
            for participant in [
                user,
                agent,
                ActorId::Agent(AgentId::new()),
                ActorId::User(UserId::new()),
                ActorId::Approver(Default::default()),
            ] {
                let taking_part_all_along = [EVERY_POSITION];
                assert_eq!(
                    recorded.seen_by(
                        Position::START,
                        &Reader::Participant(participant),
                        &taking_part_all_along
                    ),
                    envelope.recipients().names(participant),
                    "{kind} of {sender} for {participant}"
                );
            }
        }
    }

    /// Issue 223: an envelope recorded comes back from the history as it
    /// came in - its event, its context with the position of its entry, its
    /// time -, with the security context the channel gives it; a turn's end
    /// without its answer, which the history holds before it.
    #[test]
    fn an_envelope_recorded_comes_back_as_it_came_in() {
        let (agent, user) = (ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()));
        let context = EventContext {
            sender_id: agent,
            sender_name: "agent".to_string(),
            correlation_id: Uuid::new_v4(),
            span_depth: 2,
            agent_hops: 1,
            initiator_id: user,
            ..EventContext::new(Default::default(), Default::default(), agent, "agent")
        };
        for (sender, event) in recorded_events(agent, user) {
            let kind = event.kind();
            let context = EventContext {
                sender_id: sender,
                position: Some(Position::new(4)),
                ..context.clone()
            };
            let envelope = Envelope::new(context, event);
            let recorded = Recorded::of(&envelope, ActorId::System, Audience::Everybody)
                .unwrap_or_else(|| panic!("{kind} is recorded"));
            let again =
                recorded.envelope(Position::new(7)).unwrap_or_else(|| panic!("{kind} comes back"));
            assert_eq!(
                serde_json::to_value(&again.event).unwrap(),
                serde_json::to_value(&envelope.event).unwrap(),
                "{kind}: the event"
            );
            assert_eq!(
                (again.context.position, again.timestamp),
                (Some(Position::new(7)), envelope.timestamp),
                "{kind}: where its entry is, when it was sent"
            );
            assert_eq!(
                EventContext { position: None, ..again.context },
                EventContext { position: None, ..envelope.context.clone() },
                "{kind}: the context it came in"
            );
        }
        let answer =
            Message { sender_id: agent, turn_id: Some(TurnId::new()), ..make_message(id(1)) };
        let ended = Envelope::new(
            context,
            ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![answer] },
        );
        let again = Recorded::of(&ended, agent, Audience::Everybody)
            .and_then(|recorded| recorded.envelope(Position::START));
        assert!(
            matches!(again.map(|e| e.event), Some(ActorEvent::TurnFinished { messages, .. }) if messages.is_empty()),
            "without its answer"
        );
        assert!(
            Recorded::new(
                HistoryEntry::Fitted { channel: Classification::PUBLIC },
                ActorId::None,
                Audience::Everybody
            )
            .envelope(Position::START)
            .is_none(),
            "a fitting is no event"
        );
    }

    /// Issue 223: what is recorded to be delivered again is neither an item
    /// nor part of the conversation: a page shows none of it, and a turn
    /// given up is still the last thing.
    #[test]
    fn what_is_recorded_to_be_delivered_again_is_no_item() {
        let (agent, user) = (ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()));
        let turn = TurnId::new();
        let mut history = History::default();
        history.record(HistoryEntry::TurnGivenUp {
            turn_id: turn,
            actor: agent,
            reason: GivenUpReason::Left,
            at: Utc::now(),
        });
        let delivered_alone: Vec<(ActorId, ActorEvent)> = recorded_events(agent, user)
            .into_iter()
            .filter(|(sender, event)| {
                Envelope::new(
                    EventContext { sender_id: *sender, ..EventContext::default() },
                    event.clone(),
                )
                .entry()
                .is_some_and(|entry| {
                    !entry.is_item() && entry.changed().is_none() && entry.event().is_some()
                })
            })
            .collect();
        assert_eq!(
            delivered_alone.len(),
            14,
            "the requests, approvals and questions: {:?}",
            delivered_alone.iter().map(|(_, event)| event.kind()).collect::<Vec<_>>()
        );
        for (sender, event) in delivered_alone {
            let envelope =
                Envelope::new(EventContext { sender_id: sender, ..EventContext::default() }, event);
            assert!(history
                .enter(Recorded::of(&envelope, sender, Audience::Everybody).expect("recorded")));
        }
        assert_eq!(items_of(&history).len(), 1, "the turn given up alone");
        assert_eq!(
            history.turn_given_up_last(agent),
            Some((turn, GivenUpReason::Left)),
            "nothing of the conversation followed it"
        );
    }
}
