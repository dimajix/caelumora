//! Histories (issue 258): what was said and how turns went, item by item.

use caelumora_model::actor::ActorId;
use caelumora_model::digest::Digest;
use caelumora_model::history::{HistoryEntry, HistoryPage};
use caelumora_model::message::Message;
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
/// `{"participant_joined": {…}}`, `{"participant_left": {…}}`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryItemResponse<M> {
    Said(M),
    TurnFinished { turn_id: TurnId, actor: ActorId, at: DateTime<Utc> },
    TurnGivenUp { turn_id: TurnId, actor: ActorId, reason: GivenUpReason, at: DateTime<Utc> },
    PlanStepStarted { turn_id: TurnId, actor: ActorId, step_index: usize, total_steps: usize, description: String, at: DateTime<Utc> },
    /// An agent carried out a command in its turn (issue 269).
    CommandExecuted { turn_id: TurnId, actor: ActorId, issuer: ActorId, command: String, args: String, at: DateTime<Utc> },
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
            HistoryEntry::CommandExecuted { turn_id, actor, issuer, command, args, at } => Some(Self::CommandExecuted { turn_id, actor, issuer, command, args, at }),
            HistoryEntry::Compacted { turn_id, actor, from, to, at } => Some(Self::Compacted { turn_id, actor, from, to, at }),
            HistoryEntry::ActivityFinished { turn_id, actor, activity, output, at } => Some(Self::ActivityFinished { turn_id, actor, activity, output, at }),
            HistoryEntry::ParticipantJoined { participant, name, by, by_name, at } => Some(Self::ParticipantJoined { participant, name, by, by_name, at }),
            HistoryEntry::ParticipantLeft { participant, name, by, by_name, at } => Some(Self::ParticipantLeft { participant, name, by, by_name, at }),
            // What a newcomer was given of the history records the
            // delivery; no client shows it yet (issue 417).
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
/// first, and `earlier`, what to ask for the page before it with
/// (`?before=`) - `null` if there is none.
#[derive(Debug, Serialize)]
pub struct HistoryPageResponse<M> {
    pub items: Vec<HistoryItemResponse<M>>,
    pub earlier: Option<u64>,
}

impl<M> HistoryPageResponse<M> {
    /// `page`, its messages given as `said` makes them from the message and
    /// whether what it was produced from is recorded
    /// (`HistoryEntry::context`).
    pub fn of(page: HistoryPage, mut said: impl FnMut(Message, bool) -> M) -> Self {
        let items = page.items.into_iter().filter_map(|entry| {
            let recorded = entry.context().is_some();
            HistoryItemResponse::of(entry, |message| said(message, recorded))
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
        let command = HistoryItemResponse::<Message>::of(HistoryEntry::CommandExecuted { turn_id: turn, actor, issuer, command: "compact".to_string(), args: String::new(), at: Utc::now() }, |m| m).unwrap();
        let json = serde_json::to_value(command).unwrap();
        assert_eq!(json["command_executed"]["command"], "compact");
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
