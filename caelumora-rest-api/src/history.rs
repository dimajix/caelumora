//! Histories (issue 258): what was said and how turns went, item by item.

use caelumora_model::actor::{ActorId, Rejection, RetryTarget};
use caelumora_model::agent::RetryRefused;
use caelumora_model::digest::Digest;
use caelumora_model::history::{HistoryEntry, HistoryPage};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An item of a history (issue 258), as the API gives it: a message said -
/// `M`, the channel's `Message` or an agent's `HistoryMessageResponse` -,
/// where a step of a turn's plan began (issue 263) or how a turn ended, and
/// whose it was. Written as an object named by its kind: `{"said": …}`,
/// `{"plan_step_started": {…}}`, `{"turn_finished": {…}}`,
/// `{"turn_given_up": {…}}`, `{"command_executed": {…}}`, `{"compacted": {…}}`,
/// `{"participant_joined": {…}}`, `{"participant_left": {…}}`, and the
/// notices a reader was given (issue 417): `{"command_failed": {…}}`,
/// `{"retry_accepted": {…}}`, `{"retry_rejected": {…}}`,
/// `{"chain_cut_off": {…}}`, `{"message_rejected": {…}}`,
/// `{"response_requested": {…}}`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryItemResponse<M> {
    Said(M),
    TurnFinished { turn_id: TurnId, actor: ActorId, at: DateTime<Utc> },
    TurnGivenUp { turn_id: TurnId, actor: ActorId, reason: GivenUpReason, at: DateTime<Utc> },
    PlanStepStarted { turn_id: TurnId, actor: ActorId, step_index: usize, total_steps: usize, description: String, at: DateTime<Utc> },
    /// A command was carried out (issues 269, 417): by an agent in its turn
    /// `turn_id`, or by the session without one; what it answered, to `to`
    /// alone if that is private.
    CommandExecuted { turn_id: Option<TurnId>, actor: ActorId, issuer: ActorId, command: String, args: String, output: String, to: Option<ActorId>, at: DateTime<Utc> },
    /// An agent replaced its context `from` by `to` in its turn (issue 16);
    /// in an agent's history only.
    Compacted { turn_id: TurnId, actor: ActorId, from: Digest, to: Digest, at: DateTime<Utc> },
    /// An agent ended an activity of its own accord in its turn, and what
    /// came of it (issue 284).
    ActivityFinished { turn_id: TurnId, actor: ActorId, activity: String, output: String, at: DateTime<Utc> },
    /// Someone joined the channel, by the act of `by` (issue 408); the
    /// names as they were then.
    ParticipantJoined { participant: ActorId, name: String, by: ActorId, by_name: String, at: DateTime<Utc> },
    /// Someone left the channel, as `ParticipantJoined` says.
    ParticipantLeft { participant: ActorId, name: String, by: ActorId, by_name: String, at: DateTime<Utc> },
    /// An agent did not carry out a command of `to`'s, for `reason` (issue
    /// 269).
    CommandFailed { turn_id: Option<TurnId>, actor: ActorId, command: String, reason: String, to: ActorId, at: DateTime<Utc> },
    /// An agent answers again instead of `target` (issue 234).
    RetryAccepted { target: RetryTarget, actor: ActorId, at: DateTime<Utc> },
    /// An agent does not answer again instead of `target`, for `reason`.
    RetryRejected { target: RetryTarget, reason: RetryRefused, actor: ActorId, at: DateTime<Utc> },
    /// A chain between agents reached its limit (issue 200).
    ChainCutOff { message: MessageId, max_agent_hops: u32, not_asked: Vec<ActorId>, at: DateTime<Utc> },
    /// What the reader sent was not let into the channel (issue 304).
    MessageRejected { rejected: String, message: Option<MessageId>, reason: Rejection, at: DateTime<Utc> },
    /// The reader was asked to respond, to what `by` said or nudged (issues
    /// 278, 417).
    ResponseRequested { by: ActorId, at: DateTime<Utc> },
}

/// An item of a page of a history (issue 417): the item, and `seq`, the
/// position of the entry that added it - what a client tells it by, and
/// the live event that announced it (the envelope's `position`). Written as
/// the item's object with `"seq"` beside its kind: `{"seq": 7, "said": …}`.
#[derive(Debug, Serialize)]
pub struct HistoryPageItem<M> {
    pub seq: u64,
    #[serde(flatten)]
    pub item: HistoryItemResponse<M>,
}

impl<M> HistoryItemResponse<M> {
    /// The item `entry` of a history (`History::items`), its message
    /// given as `said` makes it; `None` for an entry a reader of a history
    /// is not given - a change, which the message it changed holds.
    pub fn of(entry: HistoryEntry, said: impl FnOnce(Message) -> M) -> Option<Self> {
        match entry {
            // What an agent was given reads, in its history, as it did
            // before its history was its channel's (issue 417).
            HistoryEntry::Said { message } | HistoryEntry::Given { message } => Some(Self::Said(said(message))),
            HistoryEntry::TurnFinished { turn_id, actor, at } => Some(Self::TurnFinished { turn_id, actor, at }),
            HistoryEntry::TurnGivenUp { turn_id, actor, reason, at } => Some(Self::TurnGivenUp { turn_id, actor, reason, at }),
            HistoryEntry::PlanStepStarted { turn_id, actor, step_index, total_steps, description, at } => Some(Self::PlanStepStarted { turn_id, actor, step_index, total_steps, description, at }),
            HistoryEntry::CommandExecuted { turn_id, actor, issuer, command, args, output, to, at } => Some(Self::CommandExecuted { turn_id, actor, issuer, command, args, output, to, at }),
            HistoryEntry::Compacted { turn_id, actor, from, to, at } => Some(Self::Compacted { turn_id, actor, from, to, at }),
            HistoryEntry::ActivityFinished { turn_id, actor, activity, output, at } => Some(Self::ActivityFinished { turn_id, actor, activity, output, at }),
            HistoryEntry::ParticipantJoined { participant, name, by, by_name, at } => Some(Self::ParticipantJoined { participant, name, by, by_name, at }),
            HistoryEntry::ParticipantLeft { participant, name, by, by_name, at } => Some(Self::ParticipantLeft { participant, name, by, by_name, at }),
            HistoryEntry::CommandFailed { turn_id, actor, command, reason, to, at } => Some(Self::CommandFailed { turn_id, actor, command, reason, to, at }),
            HistoryEntry::RetryAccepted { target, actor, at } => Some(Self::RetryAccepted { target, actor, at }),
            HistoryEntry::RetryRejected { target, reason, actor, at } => Some(Self::RetryRejected { target, reason, actor, at }),
            HistoryEntry::ChainCutOff { message, max_agent_hops, not_asked, at } => Some(Self::ChainCutOff { message, max_agent_hops, not_asked, at }),
            HistoryEntry::MessageRejected { rejected, message, reason, at } => Some(Self::MessageRejected { rejected, message, reason, at }),
            HistoryEntry::ResponseRequested { by, at } => Some(Self::ResponseRequested { by, at }),
            // What a newcomer was given of the history records a delivery;
            // no client shows it (issue 417).
            HistoryEntry::HistoryShared { .. } => None,
            HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. } => None,
        }
    }
}

/// How many items a page of a history holds unless asked for fewer or more.
const PAGE_ITEMS: usize = 50;

/// The most items a page of a history holds.
const MAX_PAGE_ITEMS: usize = 200;

/// Which page of a history to read (issue 218): `?before=<position>` - the
/// `earlier` of the page after it; without it, the last page - and
/// `&limit=<items>`.
#[derive(Debug, Default, Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub before: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

impl HistoryQuery {
    /// How many items the page holds: as asked, between one and
    /// `MAX_PAGE_ITEMS`; `PAGE_ITEMS` if not asked.
    pub fn limit(&self) -> usize {
        self.limit.map_or(PAGE_ITEMS, |limit| limit.clamp(1, MAX_PAGE_ITEMS))
    }
}

/// A page of a history (issue 218), as the API gives it: its items, oldest
/// first, each with its position, and `earlier`, what to ask for the page
/// before it with (`?before=`) - `null` if there is none.
#[derive(Debug, Serialize)]
pub struct HistoryPageResponse<M> {
    pub items: Vec<HistoryPageItem<M>>,
    pub earlier: Option<u64>,
}

impl<M> HistoryPageResponse<M> {
    /// `page`, its messages given as `said` makes them from the message and
    /// whether what it was produced from is recorded
    /// (`HistoryEntry::context`).
    pub fn of(page: HistoryPage, mut said: impl FnMut(Message, bool) -> M) -> Self {
        let items = page.items.into_iter().filter_map(|(seq, entry)| {
            let recorded = entry.context().is_some();
            HistoryItemResponse::of(entry, |message| said(message, recorded)).map(|item| HistoryPageItem { seq, item })
        }).collect();
        Self { items, earlier: page.earlier }
    }
}

/// A message of an agent's history (issue 207) - offered to it or said by
/// it - with whether its context can be shown (`has_context`: the agent
/// produced it, and what it saw then is recorded) and whether it was passed
/// on to the agent's transcript (`in_transcript`: not if its perceive
/// processors dropped it).
#[derive(Debug, Serialize)]
pub struct HistoryMessageResponse {
    pub message: Message,
    pub has_context: bool,
    pub in_transcript: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::confidentiality::Classification;

    use caelumora_model::user::UserId;
    use caelumora_model::digest;


    #[test]
    fn a_history_is_given_as_its_messages_and_how_turns_ended() {
        use caelumora_model::turn::GivenUpReason;
        let (turn, actor) = (TurnId::new(), ActorId::default());
        let said = HistoryItemResponse::of(HistoryEntry::Said { message: Message::new(Classification::CHANNEL_DEFAULT) }, |m| m).unwrap();
        assert!(serde_json::to_value(said).unwrap()["said"]["id"].is_string(), "the message itself");
        let given_up = HistoryItemResponse::<Message>::of(HistoryEntry::TurnGivenUp { turn_id: turn, actor, reason: GivenUpReason::Failed { message: "boom".to_string() }, at: Utc::now() }, |m| m).unwrap();
        let json = serde_json::to_value(given_up).unwrap();
        assert_eq!(json["turn_given_up"]["reason"], serde_json::json!({ "kind": "failed", "message": "boom" }));
        assert_eq!(json["turn_given_up"]["turn_id"], serde_json::json!(turn));
        let step = HistoryItemResponse::<Message>::of(HistoryEntry::PlanStepStarted { turn_id: turn, actor, step_index: 1, total_steps: 3, description: "second".to_string(), at: Utc::now() }, |m| m).unwrap();
        let json = serde_json::to_value(step).unwrap();
        assert_eq!((json["plan_step_started"]["step_index"].clone(), json["plan_step_started"]["total_steps"].clone()), (serde_json::json!(1), serde_json::json!(3)));
        assert_eq!(json["plan_step_started"]["description"], "second");
        let issuer = ActorId::User(UserId::new());
        let command = HistoryItemResponse::<Message>::of(HistoryEntry::CommandExecuted { turn_id: Some(turn), actor, issuer, command: "compact".to_string(), args: String::new(), output: "done".to_string(), to: None, at: Utc::now() }, |m| m).unwrap();
        let json = serde_json::to_value(HistoryPageItem { seq: 7, item: command }).unwrap();
        assert_eq!((json["seq"].clone(), json["command_executed"]["command"].clone(), json["command_executed"]["output"].clone()), (serde_json::json!(7), serde_json::json!("compact"), serde_json::json!("done")), "beside its kind, its position");
        assert_eq!(json["command_executed"]["issuer"], serde_json::to_value(issuer).unwrap());
        let (from, to) = (digest::content(b"before"), digest::content(b"after"));
        let compacted = HistoryItemResponse::<Message>::of(HistoryEntry::Compacted { turn_id: turn, actor, from, to, at: Utc::now() }, |m| m).unwrap();
        let json = serde_json::to_value(compacted).unwrap();
        assert_eq!((json["compacted"]["from"].clone(), json["compacted"]["to"].clone()), (serde_json::to_value(from).unwrap(), serde_json::to_value(to).unwrap()));
        let finished = HistoryItemResponse::<Message>::of(HistoryEntry::ActivityFinished { turn_id: turn, actor, activity: "compacting its context".to_string(), output: "done".to_string(), at: Utc::now() }, |m| m).unwrap();
        let json = serde_json::to_value(finished).unwrap();
        assert_eq!((json["activity_finished"]["activity"].clone(), json["activity_finished"]["output"].clone()), (serde_json::json!("compacting its context"), serde_json::json!("done")));
        let joined = HistoryItemResponse::<Message>::of(HistoryEntry::ParticipantJoined { participant: issuer, name: "bob".to_string(), by: actor, by_name: "alice".to_string(), at: Utc::now() }, |m| m).unwrap();
        let json = serde_json::to_value(joined).unwrap();
        assert_eq!((json["participant_joined"]["name"].clone(), json["participant_joined"]["by_name"].clone()), (serde_json::json!("bob"), serde_json::json!("alice")));
        let left = HistoryItemResponse::<Message>::of(HistoryEntry::ParticipantLeft { participant: issuer, name: "bob".to_string(), by: issuer, by_name: "bob".to_string(), at: Utc::now() }, |m| m).unwrap();
        assert_eq!(serde_json::to_value(left).unwrap()["participant_left"]["by"], serde_json::to_value(issuer).unwrap());
        assert!(HistoryItemResponse::<Message>::of(HistoryEntry::Fitted { channel: Classification::PUBLIC }, |m| m).is_none(), "a change is in the message it changed");
    }

}
