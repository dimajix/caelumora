//! What happened in a channel, as its history keeps it (`HistoryEntry`): a message said, a change
//! of one, how a turn ended, who joined or left - with what each is to a reader of the
//! conversation, how it changes the message it is about, and whom it goes to.

use super::Recipients;
use crate::actor::{ActorEvent, ActorId, Rejection, RequestReason, RetryTarget};
use crate::agent::RetryRefused;
use crate::approval::{
    ApprovalDecision, ApprovalId, ApprovalRequest, ApprovalStatus, ApprovalVerdict,
};
use crate::chain::Position;
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::question::{QuestionId, QuestionRequest};
use crate::turn::GivenUpReason;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What happened in a `History`; its node carries the digest of the
/// history up to and including it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A message said, with what it was produced from if a turn of its
    /// sender produced it (`Message::produced_from`, issue 417).
    Said { message: Message },
    /// A message an agent was given to read (issues 22, 417) - a skill
    /// loaded for it, what a compaction gave again -: what the agent read,
    /// not what anyone said, so no message said in the channel, found by
    /// no id and changed by nobody. Gone to nobody: its record, read by
    /// whom may read what the agent saw.
    Given { message: Message },
    /// The agent no longer has the message `message` to read, given to it
    /// before, from `at` on (issue 537) - a prompt part that no longer
    /// holds -: where it stopped reading it, not a change of it, since what
    /// it was given is changed by nobody. Gone to nobody, as what it was
    /// given, and read where that is.
    Withdrawn { message: MessageId, at: DateTime<Utc> },
    /// The message `message` - on whichever branch - is classified
    /// `classification` from here on (issue 133).
    Reclassified { message: MessageId, classification: Classification },
    /// The message `message.id` - on whichever branch - reads as `message`
    /// from here on (issue 234, `Message::modify`).
    Modified { message: Message },
    /// The message `message` - on whichever branch - was taken back by its
    /// author at `at` (issue 234, `Message::revoke`).
    Revoked { message: MessageId, at: DateTime<Utc> },
    /// The channel was classified `channel`, and every message said so far
    /// fits into it from here on (`Classification::fitted_into`).
    Fitted { channel: Classification },
    /// The turn `turn_id` of `actor` ended at `at` and was taken over
    /// (issue 258).
    TurnFinished { turn_id: TurnId, actor: ActorId, at: DateTime<Utc> },
    /// The turn `turn_id` of `actor` was given up at `at`, for `reason`
    /// (issue 258): what it said before stays said. Whose turn it was is
    /// recorded, since a turn may end before it said anything.
    TurnGivenUp { turn_id: TurnId, actor: ActorId, reason: GivenUpReason, at: DateTime<Utc> },
    /// Step `step_index` of `total_steps` of the plan of `actor`'s turn
    /// `turn_id` began at `at` (issue 263): what the turn says from here on
    /// is said in that step, until the next one begins or the turn gives
    /// its final answer. `description` is the step as the plan words it.
    PlanStepStarted {
        turn_id: TurnId,
        actor: ActorId,
        step_index: usize,
        total_steps: usize,
        description: String,
        at: DateTime<Utc>,
    },
    /// `actor` carried out the command `command` with `args` at `at`, on the
    /// word of `issuer`, and answered `output` (issues 269, 417) - to `to`
    /// alone if it is private, to every user otherwise: an agent in its turn
    /// `turn_id`, recorded before the turn's end; the session (`/agent wakeup`),
    /// without a turn, in the name of whoever asked.
    CommandExecuted {
        turn_id: Option<TurnId>,
        actor: ActorId,
        issuer: ActorId,
        command: String,
        args: String,
        output: String,
        to: Option<ActorId>,
        at: DateTime<Utc>,
    },
    /// `actor`, an agent, replaced its context in its turn `turn_id` at
    /// `at` (issue 16): `from`, the transcript as it stood, by `to`, the
    /// head of a branch of its own that begins anew - what replaces the
    /// older messages, then those kept. Every reference recorded before
    /// names the old branch, which stays; what is said afterwards is
    /// produced from the new one. What replaced the older messages follows
    /// it, as messages given to the agent. Kept in the agent's history
    /// alone: the transcript's digests do not record where a branch came
    /// from.
    Compacted { turn_id: TurnId, actor: ActorId, from: Digest, to: Digest, at: DateTime<Utc> },
    /// `actor`, an agent, ended `activity` - something of its own accord,
    /// besides answering - in its turn `turn_id` at `at` (issue 284), and
    /// `output` says what came of it: recorded where it happened, with what
    /// it said, which is everybody's.
    ActivityFinished {
        turn_id: TurnId,
        actor: ActorId,
        activity: String,
        output: String,
        at: DateTime<Utc>,
    },
    /// `participant`, called `name` then, joined the channel at `at`, by the act of `by`, called
    /// `by_name` then - the participant itself if it joined by itself, `ActorId::System` if the
    /// kernel added it (issue 408): recorded where it happened among what was said, with the names
    /// as they were, so that it reads the same after a renaming.
    ParticipantJoined {
        participant: ActorId,
        name: String,
        by: ActorId,
        by_name: String,
        at: DateTime<Utc>,
    },
    /// `participant` left the channel, as `ParticipantJoined` says - `by`
    /// who removed it, the participant itself if it left by itself. It was
    /// given nothing from the position `until` on (issue 417): where the
    /// channel stopped delivering to it, which may lie before this entry -
    /// what it still sent until its task ended comes first (issue 262).
    ParticipantLeft {
        participant: ActorId,
        name: String,
        by: ActorId,
        by_name: String,
        until: Position,
        at: DateTime<Utc>,
    },
    /// `participant` was given the history from the position `from` on as
    /// it joined at `at`, by the act of `by`, called `by_name` then (issues
    /// 46, 417): how much `by` granted (`HistoryAccess`), fixed as a
    /// position. Recorded in the system's name right before its
    /// `ParticipantJoined`, for it alone, and only if it shares something -
    /// if `from` lies before the join; otherwise what it is given begins
    /// with its join.
    HistoryShared {
        participant: ActorId,
        from: Position,
        by: ActorId,
        by_name: String,
        at: DateTime<Utc>,
    },
    /// `actor`, an agent, did not carry out the command `command` asked by
    /// `to`, for `reason` (issues 269, 417), told to `to` alone at `at`;
    /// `turn_id` if it had begun it. Nothing it did stays.
    CommandFailed {
        turn_id: Option<TurnId>,
        actor: ActorId,
        command: String,
        reason: String,
        to: ActorId,
        at: DateTime<Utc>,
    },
    /// `actor`, an agent, answers again instead of `target` (issues 234,
    /// 417), told to the users at `at`.
    RetryAccepted { target: RetryTarget, actor: ActorId, at: DateTime<Utc> },
    /// `actor`, an agent, does not answer again instead of `target`, for
    /// `reason` (issues 234, 417), told to the users at `at`.
    RetryRefused { target: RetryTarget, reason: RetryRefused, actor: ActorId, at: DateTime<Utc> },
    /// A chain between agents reached its limit at `at` (issues 200, 417):
    /// the response ending in `message` asked nobody to respond, as more
    /// than `max_agent_hops` hops since the last user's message; `not_asked`
    /// are the agents it mentioned. Told by the moderator, in the system's
    /// name, to whom it names.
    ChainCutOff {
        message: MessageId,
        max_agent_hops: u32,
        not_asked: Vec<ActorId>,
        at: DateTime<Utc>,
    },
    /// The responder policy could not choose who responds to the response ending in `message` at
    /// `at` (issue 441), and nobody was asked. Told by the moderator, in the system's name, to the
    /// users.
    ResponderUndecided { message: MessageId, at: DateTime<Utc> },
    /// What its sender sent - an event of the kind `rejected`, about
    /// `message` if it is one - was not let into the channel at `at`, for
    /// `reason` (issues 304, 417): told to the sender alone, in the system's
    /// name. What was rejected entered nothing and is not recorded.
    EventRejected {
        rejected: String,
        message: Option<MessageId>,
        reason: Rejection,
        at: DateTime<Utc>,
    },
    /// `by`, an actor, did not take what `to` sent it - an event of the kind `refused`, about
    /// `message` if it is one - at `at`, for `reason` (issue 567): told to `to` alone.
    EventRefused {
        by: ActorId,
        to: ActorId,
        refused: String,
        message: Option<MessageId>,
        reason: Rejection,
        at: DateTime<Utc>,
    },
    /// Whom it went to was asked to respond at `at` (issue 417) - to what
    /// `by` said or nudged -, as the moderator decided and for `reason`
    /// (issue 439): in the system's name, to those asked.
    ResponseRequested { by: ActorId, reason: RequestReason, at: DateTime<Utc> },
    /// `by`, a user, asked the agent `agent` at `at` to carry out the
    /// command `command` with `args` (issues 269, 223). Like every entry
    /// that follows, recorded to be delivered again from the history - to
    /// an agent whose inbox fell behind, or that starts again before it
    /// stored what it took in -, and no item: shown live alone.
    CommandRequested {
        agent: ActorId,
        command: String,
        args: String,
        by: ActorId,
        at: DateTime<Utc>,
    },
    /// `by`, a user, asked `author`, an agent, at `at` to answer again
    /// instead of `target` (issues 234, 223).
    RetryRequested { target: RetryTarget, author: ActorId, by: ActorId, at: DateTime<Utc> },
    /// `by` asked at `at` for a decision on `request` (issues 81, 223): an
    /// agent, of the approvers; an approver, of the users.
    ApprovalRequested { request: ApprovalRequest, by: ActorId, at: DateTime<Utc> },
    /// `by`, a user, answered the request `id` at `at` (issues 81, 419, 567): whether they approve,
    /// with their part (`decision`) - to the approvers, which hold the request.
    ApprovalAnswered {
        id: ApprovalId,
        approved: bool,
        decision: ApprovalDecision,
        by: ActorId,
        at: DateTime<Utc>,
    },
    /// `by`, an approver, decided the request `response.id` with `status` at `at` (issues 81, 419,
    /// 223): to the users and the agent that asked.
    ApprovalDecided {
        response: ApprovalVerdict,
        status: ApprovalStatus,
        by: ActorId,
        at: DateTime<Utc>,
    },
    /// `by` withdrew the request `id` at `at` (issues 148, 223): the agent
    /// that asked, of the approvers; an approver, of the users.
    ApprovalRequestWithdrawn { id: ApprovalId, by: ActorId, at: DateTime<Utc> },
    /// `by`, an agent, asked the questions `request` at `at` (issues 165,
    /// 223). The answers are no entry: they go to the tool alone, never
    /// into a history.
    QuestionRequested { request: QuestionRequest, by: ActorId, at: DateTime<Utc> },
    /// `by`, an agent, no longer waits for the answers to its questions
    /// `id` since `at` (issues 165, 223), told to whom they were shown.
    QuestionClosed {
        id: QuestionId,
        shown_to: Option<Vec<ActorId>>,
        by: ActorId,
        at: DateTime<Utc>,
    },
}

impl AsRef<HistoryEntry> for HistoryEntry {
    fn as_ref(&self) -> &HistoryEntry {
        self
    }
}

impl HistoryEntry {
    /// Whether it is an item of its history - a message said or given, where
    /// an agent stopped reading one given, the start of a plan's step, a
    /// command carried out or not, an activity ended, a compaction, a turn's
    /// end, a change of who takes part, a notice, a request - rather than a
    /// change of what was said before:
    /// what a reader of the history is given as it stands
    /// (`StandingItems`), a change being in the message it changes. What is
    /// recorded only to be delivered again (issue 223) - a command or a
    /// request to answer again asked of an agent, approvals and questions -
    /// is neither: never in a page, shown live alone.
    pub fn is_item(&self) -> bool {
        match self {
            HistoryEntry::Said { .. }
            | HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::EventRefused { .. }
            | HistoryEntry::ResponseRequested { .. } => true,
            HistoryEntry::Reclassified { .. }
            | HistoryEntry::Modified { .. }
            | HistoryEntry::Revoked { .. }
            | HistoryEntry::Fitted { .. } => false,
            // Recorded to be delivered, not read (issue 223): neither an item
            // nor a change.
            HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => false,
        }
    }

    /// Whether it is part of the conversation - something said, given or
    /// done in it, a change of it, a turn's end, a change of who takes
    /// part -, rather than what was only told or asked about it: a notice,
    /// a request to respond, what a newcomer was given. What "the last
    /// thing" in a channel is read among (issue 417).
    pub fn is_conversation(&self) -> bool {
        match self {
            HistoryEntry::Said { .. }
            | HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::Reclassified { .. }
            | HistoryEntry::Modified { .. }
            | HistoryEntry::Revoked { .. }
            | HistoryEntry::Fitted { .. } => true,
            HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::EventRefused { .. }
            | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => false,
        }
    }

    /// Whether it ends a turn of `actor`, finished or given up.
    pub fn ends_turn_of(&self, actor: ActorId) -> bool {
        matches!(self, HistoryEntry::TurnFinished { actor: of, .. } | HistoryEntry::TurnGivenUp { actor: of, .. } if *of == actor)
    }

    /// The message, if this is one said.
    pub fn said(&self) -> Option<&Message> {
        match self {
            HistoryEntry::Said { message } => Some(message),
            HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::Reclassified { .. }
            | HistoryEntry::Modified { .. }
            | HistoryEntry::Revoked { .. }
            | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::EventRefused { .. }
            | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => None,
        }
    }

    /// Applies what it changes to `standing`, a message said before it as it
    /// stands, and says whether that changed it: a modification, a
    /// revocation or a reclassification of that message
    /// (`Message::modify`, `revoke`, `reclassify`), and a fitting of the
    /// channel's, which applies to every message before it
    /// (`Classification::fitted_into`). Every other entry changes nothing.
    /// The one rule by which what stands is read from a history.
    pub fn fold_into(&self, standing: &mut Message) -> bool {
        match self {
            HistoryEntry::Modified { message } if message.id == standing.id => {
                standing.modify(message)
            }
            HistoryEntry::Revoked { message, at } if *message == standing.id => {
                standing.revoke(*at)
            }
            HistoryEntry::Reclassified { message, classification } if *message == standing.id => {
                standing.reclassify(classification)
            }
            HistoryEntry::Fitted { channel } => standing.fit_into(channel),
            HistoryEntry::Said { .. }
            | HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::Modified { .. }
            | HistoryEntry::Revoked { .. }
            | HistoryEntry::Reclassified { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::EventRefused { .. }
            | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => false,
        }
    }

    /// The message it modifies, takes back or reclassifies, if it does.
    pub fn changed(&self) -> Option<MessageId> {
        match self {
            HistoryEntry::Modified { message } => Some(message.id),
            HistoryEntry::Revoked { message, .. } | HistoryEntry::Reclassified { message, .. } => {
                Some(*message)
            }
            HistoryEntry::Said { .. }
            | HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::EventRefused { .. }
            | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => None,
        }
    }

    /// Whom the entry goes to when it goes to everybody, by the rule every
    /// channel delivers by (issue 417, `Envelope::recipients`): a message to
    /// every participant who is a user, and to every agent too if there is
    /// something to hear in it; a change of a message and who joined or left
    /// to every user and agent; how a turn went, what it carried out and
    /// what came of what it did of its own accord to the users; what
    /// answers a command to the user it answers; the part of the history
    /// shared with a newcomer to the newcomer alone; what an agent was given,
    /// what it no longer has to read and a compaction to nobody; a command
    /// and a request to answer again to the agent they name (issues 234,
    /// 269); an approval through the approvers - an agent's request to them,
    /// theirs to the users, a user's answer to them, their decision to the
    /// users and the agent that asked (issues 81, 148); an agent's questions
    /// and their closing to the users, to those asked alone if they are
    /// sensitive (issue 165). Whether it went back to its sender is not this
    /// rule's: a reader is given what it sent itself.
    pub fn recipients(&self) -> Recipients {
        match self {
            HistoryEntry::HistoryShared { participant, .. } => Recipients::only(vec![*participant]),
            HistoryEntry::CommandExecuted { to: Some(to), .. }
            | HistoryEntry::CommandFailed { to, .. } => Recipients::only(
                vec![*to].into_iter().filter(|to| matches!(to, ActorId::User(_))).collect(),
            ),
            HistoryEntry::Said { message } => Recipients::participants(message.is_heard()),
            HistoryEntry::Modified { .. }
            | HistoryEntry::Revoked { .. }
            | HistoryEntry::Reclassified { .. }
            | HistoryEntry::Fitted { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. } => Recipients::participants(true),
            HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::CommandExecuted { to: None, .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. } => Recipients::participants(false),
            HistoryEntry::Compacted { .. }
            | HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::ResponseRequested { .. } => Recipients::NOBODY,
            HistoryEntry::EventRefused { to, .. } => Recipients::only(vec![*to]),
            HistoryEntry::CommandRequested { agent: named, .. }
            | HistoryEntry::RetryRequested { author: named, .. } => Recipients::only(
                Some(*named)
                    .filter(|named| matches!(named, ActorId::Agent(_)))
                    .into_iter()
                    .collect(),
            ),
            HistoryEntry::ApprovalRequested { by, .. }
            | HistoryEntry::ApprovalRequestWithdrawn { by, .. } => match by {
                ActorId::Agent(_) => Recipients::APPROVERS,
                ActorId::Approver(_) => Recipients::participants(false),
                _ => Recipients::NOBODY,
            },
            HistoryEntry::ApprovalAnswered { by: ActorId::User(_), .. } => Recipients::APPROVERS,
            HistoryEntry::ApprovalAnswered { .. } => Recipients::NOBODY,
            HistoryEntry::ApprovalDecided { response, by, .. } => match by {
                ActorId::Approver(_) => Recipients {
                    named: vec![ActorId::Agent(response.agent_id)],
                    ..Recipients::participants(false)
                },
                _ => Recipients::NOBODY,
            },
            HistoryEntry::QuestionRequested { by: ActorId::Agent(_), request, .. } => {
                Recipients::users(request.shown_to())
            }
            HistoryEntry::QuestionClosed { by: ActorId::Agent(_), shown_to, .. } => {
                Recipients::users(shown_to.clone())
            }
            HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => {
                Recipients::NOBODY
            }
        }
    }

    /// The event the entry records, as it is delivered again from the
    /// history (issue 223): the inverse of `Envelope::entry`, whose
    /// envelope gives the rest - who sent it, when, who caused it. A message
    /// said is an intermediate one if it is internal, a step of a plan, as
    /// an agent sends it, and a complete one otherwise; a turn's end comes
    /// without its answer, which the history holds as the messages said
    /// before it (`HistoryStore::answer`). `None` for a fitting of the
    /// channel's, which is no event.
    pub fn event(&self) -> Option<ActorEvent> {
        Some(match self.clone() {
            HistoryEntry::Said { message } if message.internal => {
                ActorEvent::IntermediateMessageGenerated(message)
            }
            HistoryEntry::Said { message } => ActorEvent::CompleteMessageGenerated(message),
            HistoryEntry::Given { message } => ActorEvent::MessageGiven { message },
            HistoryEntry::Withdrawn { message, at } => ActorEvent::MessageWithdrawn { message, at },
            HistoryEntry::Reclassified { message, classification } => {
                ActorEvent::MessageReclassified { message, classification }
            }
            HistoryEntry::Modified { message } => ActorEvent::MessageModified(message),
            HistoryEntry::Revoked { message, at } => ActorEvent::MessageRevoked { message, at },
            HistoryEntry::TurnFinished { turn_id, .. } => {
                ActorEvent::TurnFinished { turn_id, messages: Vec::new() }
            }
            HistoryEntry::TurnGivenUp { turn_id, reason, .. } => {
                ActorEvent::TurnGivenUp { turn_id, reason }
            }
            HistoryEntry::PlanStepStarted {
                turn_id, step_index, total_steps, description, ..
            } => ActorEvent::PlanStepStarted { turn_id, step_index, total_steps, description },
            HistoryEntry::CommandExecuted { turn_id, command, args, output, to, .. } => {
                ActorEvent::CommandExecuted { command, args, output, to, turn_id }
            }
            HistoryEntry::Compacted { turn_id, from, to, .. } => {
                ActorEvent::ContextCompacted { turn_id, from, to }
            }
            HistoryEntry::ActivityFinished { turn_id, activity, output, .. } => {
                ActorEvent::ActivityFinished { turn_id, activity, output }
            }
            HistoryEntry::ParticipantJoined { participant, name, .. } => {
                ActorEvent::ParticipantJoined { participant, name }
            }
            HistoryEntry::ParticipantLeft { participant, name, until, .. } => {
                ActorEvent::ParticipantLeft { participant, name, until }
            }
            HistoryEntry::HistoryShared { participant, from, by, by_name, .. } => {
                ActorEvent::HistoryShared { participant, from, by, by_name }
            }
            HistoryEntry::CommandFailed { turn_id, command, reason, to, .. } => {
                ActorEvent::CommandFailed { command, reason, to, turn_id }
            }
            HistoryEntry::RetryAccepted { target, .. } => ActorEvent::RetryAccepted { target },
            HistoryEntry::RetryRefused { target, reason, .. } => {
                ActorEvent::RetryRefused { target, reason }
            }
            HistoryEntry::ChainCutOff { message, max_agent_hops, not_asked, .. } => {
                ActorEvent::ChainCutOff { message, max_agent_hops, not_asked }
            }
            HistoryEntry::ResponderUndecided { message, .. } => {
                ActorEvent::ResponderUndecided { message }
            }
            HistoryEntry::EventRejected { rejected, message, reason, .. } => {
                ActorEvent::EventRejected { rejected, message, reason }
            }
            HistoryEntry::EventRefused { to, refused, message, reason, .. } => {
                ActorEvent::EventRefused { to, refused, message, reason }
            }
            HistoryEntry::ResponseRequested { reason, .. } => {
                ActorEvent::ResponseRequested { reason }
            }
            HistoryEntry::CommandRequested { agent, command, args, .. } => {
                ActorEvent::CommandRequested { command, args, agent }
            }
            HistoryEntry::RetryRequested { target, author, .. } => {
                ActorEvent::RetryRequested { target, author }
            }
            HistoryEntry::ApprovalRequested { request, .. } => {
                ActorEvent::ApprovalRequested(request)
            }
            HistoryEntry::ApprovalAnswered { id, approved, decision, .. } => {
                ActorEvent::ApprovalAnswered { id, approved, decision }
            }
            HistoryEntry::ApprovalDecided {
                response, status: ApprovalStatus::Approved, ..
            } => ActorEvent::ApprovalGranted(response),
            HistoryEntry::ApprovalDecided { response, status: ApprovalStatus::Denied, .. } => {
                ActorEvent::ApprovalDenied(response)
            }
            HistoryEntry::ApprovalDecided { response, status: ApprovalStatus::Timeout, .. } => {
                ActorEvent::ApprovalTimedOut(response)
            }
            HistoryEntry::ApprovalRequestWithdrawn { id, .. } => {
                ActorEvent::ApprovalRequestWithdrawn { id }
            }
            HistoryEntry::QuestionRequested { request, .. } => {
                ActorEvent::QuestionRequested(request)
            }
            HistoryEntry::QuestionClosed { id, shown_to, .. } => {
                ActorEvent::QuestionClosed { id, shown_to }
            }
            HistoryEntry::Fitted { .. } => return None,
        })
    }

    /// Whether anything of the entry is for others than its sender
    /// (`published`): what a store keeps of it as it stands, and filters by.
    pub fn is_published(&self) -> bool {
        self.published().is_some()
    }

    /// The entry as everybody but its sender may see it (issue 417): a
    /// message, said or modified, as it is published (`Message::published`),
    /// `None` if nothing of it is left to show; nothing of what an agent was
    /// given, nor of what it no longer has to read; every other entry as it
    /// is.
    pub fn published(&self) -> Option<HistoryEntry> {
        match self {
            HistoryEntry::Said { message } => {
                message.published().map(|message| HistoryEntry::Said { message })
            }
            HistoryEntry::Modified { message } => {
                message.published().map(|message| HistoryEntry::Modified { message })
            }
            HistoryEntry::Given { .. } | HistoryEntry::Withdrawn { .. } => None,
            HistoryEntry::Revoked { .. }
            | HistoryEntry::Reclassified { .. }
            | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::EventRefused { .. }
            | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => Some(self.clone()),
        }
    }

    /// Whom the entry names as having done what it records: the sender of a
    /// message said or modified, the actor of a turn's mark, whoever added
    /// or removed a participant; nobody known for a revocation, a
    /// withdrawal, a reclassification or a fitting. What a holder records as
    /// the sender of what it was told or did itself (`Recorded::delivered`).
    pub(super) fn named_sender(&self) -> ActorId {
        match self {
            HistoryEntry::Said { message }
            | HistoryEntry::Given { message }
            | HistoryEntry::Modified { message } => message.sender_id,
            HistoryEntry::TurnFinished { actor, .. }
            | HistoryEntry::TurnGivenUp { actor, .. }
            | HistoryEntry::PlanStepStarted { actor, .. }
            | HistoryEntry::CommandExecuted { actor, .. }
            | HistoryEntry::Compacted { actor, .. }
            | HistoryEntry::ActivityFinished { actor, .. }
            | HistoryEntry::CommandFailed { actor, .. }
            | HistoryEntry::RetryAccepted { actor, .. }
            | HistoryEntry::RetryRefused { actor, .. } => *actor,
            HistoryEntry::ParticipantJoined { by, .. }
            | HistoryEntry::ParticipantLeft { by, .. }
            | HistoryEntry::HistoryShared { by, .. }
            | HistoryEntry::CommandRequested { by, .. }
            | HistoryEntry::RetryRequested { by, .. }
            | HistoryEntry::ApprovalRequested { by, .. }
            | HistoryEntry::ApprovalAnswered { by, .. }
            | HistoryEntry::ApprovalDecided { by, .. }
            | HistoryEntry::EventRefused { by, .. }
            | HistoryEntry::ApprovalRequestWithdrawn { by, .. }
            | HistoryEntry::QuestionRequested { by, .. }
            | HistoryEntry::QuestionClosed { by, .. } => *by,
            HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::ResponseRequested { .. } => ActorId::System,
            HistoryEntry::Revoked { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::Reclassified { .. }
            | HistoryEntry::Fitted { .. } => ActorId::None,
        }
    }

    /// What the message it says was produced from - the digests of the
    /// transcript before it and of the frame around it (issue 426) -, and in
    /// which turn, if this is one said and both are known: whether what its
    /// author saw can be shown (issue 207).
    pub fn context(&self) -> Option<(Option<Digest>, Digest, TurnId)> {
        match self {
            HistoryEntry::Said { message } => {
                let (transcript, frame) = message.produced_from()?;
                Some((transcript, frame, message.turn_id?))
            }
            HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::Reclassified { .. }
            | HistoryEntry::Modified { .. }
            | HistoryEntry::Revoked { .. }
            | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRefused { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::EventRejected { .. }
            | HistoryEntry::EventRefused { .. }
            | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Confidentiality;
    use crate::context::FrameRef;
    use crate::digest;
    use crate::digest::message_chain;
    use crate::history::testing::recorded_context;
    use crate::history::History;
    use crate::message::testing::{id, make_message};

    #[test]
    fn an_entry_folds_into_the_message_it_changes_and_says_whether_it_did() {
        let at = Utc::now();
        let mut message = make_message(id(1));
        assert!(
            !HistoryEntry::Revoked { message: id(2), at }.fold_into(&mut message),
            "another message"
        );
        assert!(
            !HistoryEntry::Said { message: make_message(id(1)) }.fold_into(&mut message),
            "says, changes nothing"
        );
        assert!(HistoryEntry::Modified {
            message: Message { content: Some("changed".to_string()), ..make_message(id(1)) }
        }
        .fold_into(&mut message));
        assert_eq!(message.content.as_deref(), Some("changed"));
        let fitted =
            HistoryEntry::Fitted { channel: Classification::general(Confidentiality::Public) };
        assert!(fitted.fold_into(&mut message), "lowered to the channel's");
        assert!(!fitted.fold_into(&mut message), "fits already");
        assert!(HistoryEntry::Revoked { message: id(1), at }.fold_into(&mut message));
        assert!(
            !HistoryEntry::Revoked { message: id(1), at }.fold_into(&mut message),
            "taken back already"
        );
    }

    #[test]
    fn a_history_keeps_what_its_messages_were_produced_from() {
        let transcript = Some(message_chain(None, &make_message(id(9))));
        let frame = digest::frame(&FrameRef::default());
        let turn = TurnId::new();
        let mut history = History::default();
        history.extend(vec![make_message(id(1))]);
        history.push(
            Message { turn_id: Some(turn), ..make_message(id(2)) }
                .produced_from_context(transcript, frame),
        );

        let loaded: History =
            serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        let entries: Vec<_> = loaded.all_entries().collect();
        assert_eq!(entries[0].entry.entry.context(), None, "said by someone else");
        assert_eq!(
            entries[1].entry.entry.context(),
            Some((transcript, frame, turn)),
            "with the turn it was produced in"
        );
        assert_eq!(
            recorded_context(&loaded, id(2)),
            Some((transcript, frame, turn)),
            "found by its message"
        );
        assert_eq!(recorded_context(&loaded, id(1)), None);
        assert_eq!(loaded.digest(), history.digest());
    }
}
