//! Channels and what is done in them: making and changing one, its members, messages,
//! approvals, answers to questions and commands.

use crate::history::HistoryItemResponse;
use crate::session::{ParticipantKind, UNKNOWN};
use caelumora_model::snapshot;
use caelumora_model::actor::{ActorId, ActorState};
use caelumora_model::agent::PersonalityKind;
use caelumora_model::approval::{ApprovalRequest, ApprovalRule};
use caelumora_model::channel::HistoryAccess;
use caelumora_model::command::Command;
use caelumora_model::confidentiality::Classification;
use caelumora_model::message::{Message, TokenUsage};
use caelumora_model::note::Note;
use caelumora_model::question::{QuestionRequest, Response};
use caelumora_model::removal::Removal;
use caelumora_model::session::ChannelId;
use caelumora_model::value::language::Language;
use caelumora_model::value::text::{Briefing, Summary};
use caelumora_model::work::TurnId;
use caelumora_model::workspace::WorkspaceId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The state of a channel when a stream connects: its members and the end of
/// its history, as items - messages and how turns ended (issue 258). Older
/// ones are fetched over REST - `has_more` says there are some.
#[derive(Debug, Serialize)]
pub struct ChannelSnapshot {
    pub channel: ChannelResponse,
    pub items: Vec<HistoryItemResponse<Message>>,
    pub has_more: bool,
    /// The notes every agent of the channel published (issue 17); a change
    /// is announced as the channel's `agent_state_changed` on the event
    /// stream, and
    /// the notes reloaded from `GET …/channels/{cid}/notes` (issue 65).
    pub notes: Vec<PublishedNotesResponse>,
    /// The approvals the channel's agents wait for (issue 172), as their
    /// turns are stored: the stream tells only of requests made while it
    /// listens. Empty for a viewer who does not take part as a user.
    pub pending_approvals: Vec<ApprovalRequest>,
    /// The questions the channel's agents wait for the answers to (issue
    /// 165), as their turns are stored - sensitive ones only for those
    /// asked. Empty for a viewer who does not take part as a user.
    pub pending_questions: Vec<QuestionRequest>,
}

impl ChannelSnapshot {
    /// The snapshot of a channel's stream: `channel`, whose agent members
    /// carry their notes and pending approvals - listed beside the channel,
    /// as the API has them - and the end of its history.
    pub fn of(mut channel: snapshot::ChannelSnapshot, items: Vec<HistoryItemResponse<Message>>, has_more: bool) -> Self {
        let mut notes = Vec::new();
        let (mut pending_approvals, mut pending_questions) = (Vec::new(), Vec::new());
        for member in &mut channel.members {
            if matches!(member.id, ActorId::Agent(_)) {
                let name = member.name.clone().unwrap_or_default();
                notes.push(PublishedNotesResponse { agent_id: member.id, name, notes: std::mem::take(&mut member.notes) });
            }
            pending_approvals.append(&mut member.pending_approvals);
            pending_questions.append(&mut member.pending_questions);
        }
        Self { channel: channel.into(), items, has_more, notes, pending_approvals, pending_questions }
    }
}

/// The notes one agent published in a channel.
#[derive(Debug, Serialize)]
pub struct PublishedNotesResponse {
    pub agent_id: ActorId,
    pub name: String,
    pub notes: Vec<Note>,
}

impl From<snapshot::PublishedNotes> for PublishedNotesResponse {
    fn from(published: snapshot::PublishedNotes) -> Self {
        Self { agent_id: published.agent, name: published.name, notes: published.notes }
    }
}

/// A participant of the session to add to a channel (issue 46), with how
/// much of the channel's history they may see - all of it unless asked.
#[derive(Debug, Deserialize)]
pub struct AddParticipantRequest {
    pub participant: ActorId,
    #[serde(default)]
    pub history: HistoryAccess,
}

/// A channel to create in a running session (issue 09).
#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    /// Made into a name where the channel is made (issues 177, 248).
    pub name: String,
    /// The kind of the moderator, e.g. `policy_moderator`; the default if
    /// absent (issue 174).
    pub moderator: Option<String>,
    /// The kind of the policy for the moderator's slot, e.g.
    /// `mentions_only_policy`; the default if absent.
    pub responder_policy: Option<String>,
    /// Participants of the session to take part. The requesting user takes
    /// part anyway.
    #[serde(default)]
    pub participants: Vec<ActorId>,
    /// The classification of the channel's contents (issue 133): a
    /// `level` - `public`, `internal`, `confidential` or
    /// `strictly_confidential` - and `compartments`, which must be
    /// configured; `internal` without compartments if absent.
    pub classification: Option<Classification>,
    /// The language its conversation is held in (issue 56), a BCP 47 tag;
    /// none in particular if absent (issue 331).
    #[serde(default)]
    pub language: Option<String>,
    /// Whether its members work on the session's workspace in it (issue
    /// 253); yes if absent.
    #[serde(default)]
    pub uses_workspace: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ChannelResponse {
    pub id: ChannelId,
    pub name: String,
    /// What it is for, in a line (issue 245); absent if nobody said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// What an agent should know before it works here (issue 245): given
    /// where one session or channel is asked for, not where they are
    /// listed; absent if nobody wrote one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// All participants, in no particular order, with the handles they are
    /// `@`-mentioned by - used by the web UI for the member list, completion
    /// and to render the `mentions` of a message.
    pub members: Vec<ChannelMemberResponse>,
    /// The kind of the moderator, e.g. `policy_moderator` (issue 174);
    /// `null` if it is a preconfigured component.
    pub moderator: Option<String>,
    /// The kind of the policy for the moderator's slot, e.g.
    /// `round_robin_policy` or `mentions_only_policy` - who is asked to
    /// respond to a message (issues 44, 174); `null` like `moderator`.
    pub responder_policy: Option<String>,
    /// The classification of the channel's contents (issue 133).
    pub classification: Classification,
    /// The language its conversation is held in (issue 56); `null` for none
    /// in particular.
    pub language: Option<Language>,
    /// What it restricts of the tool calls made in it (issue 13): rules that
    /// ask or deny.
    pub approval_rules: Vec<ApprovalRule>,
    /// Whether its members work on the session's workspace here (issue
    /// 251), where the classifications let them.
    pub uses_workspace: bool,
    /// The session's workspace, if the channel uses it and you may read it:
    /// its files are at `/workspaces/{id}/files` (issue 23).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    /// Whether the requesting user may rename the channel, add or remove
    /// its participants (issue 46) and set its approval rules.
    pub can_change: bool,
    /// Who took part and left (issue 164), in the order they left, each
    /// with `left`: who removed them and when.
    pub former_members: Vec<ChannelMemberResponse>,
}

impl From<snapshot::ChannelSnapshot> for ChannelResponse {
    fn from(view: snapshot::ChannelSnapshot) -> Self {
        Self {
            id: view.id,
            name: view.name,
            summary: view.summary,
            briefing: view.briefing,
            // Every member is a user, an agent or an approver.
            members: view.members.into_iter().filter_map(ChannelMemberResponse::of).collect(),
            moderator: view.moderator,
            responder_policy: view.responder_policy,
            classification: view.classification,
            language: view.language,
            approval_rules: view.approval_rules,
            uses_workspace: view.uses_workspace,
            workspace_id: view.workspace_id,
            can_change: view.can_change,
            former_members: view.former_members.into_iter().filter_map(ChannelMemberResponse::of).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ChannelMemberResponse {
    pub id: ActorId,
    pub name: String,
    /// `None` if the participant cannot be mentioned.
    pub handle: Option<String>,
    pub kind: ParticipantKind,
    /// Whether the requesting user may see what this participant saw when it
    /// produced a message (issue 41) - only ever for agents, and only as the
    /// policy decides for this user.
    pub can_read_context: bool,
    /// Whether the requesting user may see which tools this participant has
    /// now (issue 194) - only ever for agents.
    pub can_read_tools: bool,
    /// Whether the requesting user may see which components this
    /// participant was built from (issue 270) - only ever for agents, and
    /// only its owners.
    pub can_read_components: bool,
    /// Whether the requesting user may approve this participant's tool
    /// calls (issue 14) - only ever for agents taking part. Everybody sees
    /// a pending approval; only who may approve is asked.
    pub can_approve: bool,
    /// Whether the requesting user may ask this participant to answer again
    /// instead of one of its messages or of its answer under way (issues
    /// 234, 235, 242) - only ever an agent taking part that answers again,
    /// not one that runs elsewhere. Whether it does is the agent's to
    /// decide.
    pub can_retry: bool,
    /// Whether the requesting user may end this participant's answer under
    /// way (`POST …/participants/{aid}/cancel`, issues 235, 243) - only ever
    /// an agent taking part.
    pub can_cancel: bool,
    /// What the participant's model calls in the channel used (issue 209):
    /// given by the snapshot of the channel's stream and for a participant
    /// asked for alone, not where channels are listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// For a former member (issue 164): who removed it and when.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<Removal>,
    /// Whether the requesting user may delete this member - only ever an
    /// agent, its owner (issue 164).
    pub can_delete: bool,
    /// The turn of the answer the member is giving now (issue 240): what a
    /// client that connects during an answer shows as under way, as if it
    /// had seen `ResponseStarted`. Absent if none - also for an external
    /// agent, which answers without a turn here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_under_way: Option<TurnId>,
    /// Where the participant stands now (issue 243), an agent's: `Idle`,
    /// `Sleeping`, `Planning`, `ExecutingTools`, `WaitingForUser`,
    /// `Reflecting`, `Cancelling`. Kept up by `StateChanged` on the
    /// channel's stream. Absent for a user or an approver, and in a channel
    /// that does not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ActorState>,
    /// The commands an agent carries out itself that the requesting user
    /// may see (issue 269), as `GET …/commands` gives the channel's - for
    /// completion; written `/name @handle …`. Absent if there are none, and
    /// in a channel that does not run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Command>,
    /// How the participant is introduced (issues 176, 244): an agent by its
    /// personality, a user by themselves. Given for a participant asked for
    /// alone, not where channels are listed; absent if there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introduction: Option<String>,
    /// The name of an agent's personality (issue 244) - for a participant
    /// asked for alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    /// Whether an agent runs here (`internal`) or elsewhere (`external`,
    /// issue 244) - for a participant asked for alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality_kind: Option<PersonalityKind>,
    /// Who owns an agent: whose authority it acts with and who answers for
    /// its tool calls (issue 244) - for a participant asked for alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<ActorId>,
    /// The owner's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
}

impl ChannelMemberResponse {
    /// `view` as the API shows it; `None` for an id that is no participant.
    pub fn of(view: snapshot::MemberSnapshot) -> Option<Self> {
        Some(Self {
            id: view.id,
            kind: ParticipantKind::of(view.id)?,
            name: view.name.unwrap_or_else(|| UNKNOWN.to_string()),
            handle: view.handle,
            can_read_context: view.can_read_context,
            can_read_tools: view.can_read_tools,
            can_read_components: view.can_read_components,
            can_approve: view.can_approve,
            can_retry: view.can_retry,
            can_cancel: view.can_cancel,
            usage: view.usage,
            left: view.left,
            can_delete: view.can_delete,
            turn_under_way: view.turn_under_way,
            state: view.state,
            commands: view.commands,
            introduction: view.introduction,
            personality: view.personality,
            personality_kind: view.personality_kind,
            owner: view.owner,
            owner_name: view.owner_name,
        })
    }
}

/// What a user writes: a new message, or the new version of one of their
/// messages (issue 234).
#[derive(Debug, Deserialize)]
pub struct MessageRequest {
    pub content: String,
    /// How the message is classified (issue 133) - if absent, a new one as
    /// the channel is, a modified one as it is now. Another than the
    /// channel's releases it: to a wider audience or a lower level, as long
    /// as it may flow into the channel.
    pub classification: Option<Classification>,
}

/// The turn of a participant a request is about, as `ResponseStarted` and
/// its messages name it: the answer under way that is asked for again
/// (issue 235), the turn whose tool call is interrupted (issue 276).
#[derive(Debug, Deserialize)]
pub struct TurnRequest {
    pub turn_id: TurnId,
}

/// The answers of the user to questions an agent's call asks (issue 165):
/// one response per question, in their order - or `declined`, without
/// responses.
#[derive(Debug, Deserialize)]
pub struct AnswerQuestionsRequest {
    #[serde(default)]
    pub responses: Option<Vec<Response>>,
    #[serde(default)]
    pub declined: bool,
}

#[derive(Debug, Deserialize)]
pub struct SendApprovalRequest {
    pub approval_id: Uuid,
    pub agent_id: Uuid,
    pub approved: bool,
}

/// A user command to run in a channel (issue 28): the name without the slash
/// and the rest of the input line, which the command parses itself.
#[derive(Debug, Deserialize)]
pub struct ExecuteCommandRequest {
    pub name: String,
    #[serde(default)]
    pub args: String,
}

/// What a command answered. `output` is set only if the answer is for the
/// caller alone; an answer for the whole channel arrives on the channel's
/// event stream as a `CommandExecuted`, like everybody else sees it.
/// `span_id` is set for a command sent to agents (issue 269), whose answers
/// arrive on the stream in that span - `output` then names the agents that
/// were not asked, if any were left out.
#[derive(Debug, Serialize)]
pub struct CommandReplyResponse {
    pub command: String,
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<uuid::Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::DescriptionRequest;
    use caelumora_model::agent::AgentId;
    use caelumora_model::approval::ApproverId;
    use caelumora_model::user::UserId;

    #[test]
    fn a_channel_view_is_answered_with_kinds_and_a_name_for_the_deleted() {
        let (alice, approver) = (ActorId::User(UserId::new()), ActorId::Approver(ApproverId(Uuid::new_v4())));
        let member = |id, name: Option<&str>| snapshot::MemberSnapshot { id, name: name.map(str::to_string), handle: None, can_read_context: false, can_read_tools: false, can_read_components: false, can_approve: false, can_retry: false, can_cancel: false, usage: None, notes: Vec::new(), pending_approvals: Vec::new(), pending_questions: Vec::new(), left: None, can_delete: false, turn_under_way: None, state: None, commands: Vec::new(), introduction: None, personality: None, personality_kind: None, owner: None, owner_name: None };
        let view = snapshot::ChannelSnapshot { summary: None, briefing: None, approval_rules: Vec::new(), workspace_id: None,
            uses_workspace: true,
            id: ChannelId::new(),
            name: "main".to_string(),
            moderator: Some("policy_moderator".to_string()),
            responder_policy: Some("broadcast_policy".to_string()),
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            can_change: true,
            members: vec![member(alice, None), member(approver, Some("Approver")), member(ActorId::System, Some("system"))],
            former_members: Vec::new(),
        };

        let response = ChannelResponse::from(view);

        assert!(response.can_change);
        let kinds: Vec<_> = response.members.iter().map(|m| (m.kind, m.name.as_str())).collect();
        assert_eq!(kinds, [(ParticipantKind::User, UNKNOWN), (ParticipantKind::Approver, "Approver")], "the system is no participant");
    }

    #[test]
    fn what_a_place_is_for_is_left_out_where_nothing_was_said_or_read() {
        let view = snapshot::ChannelSnapshot { summary: None, briefing: None, approval_rules: Vec::new(), workspace_id: None, id: ChannelId::new(), name: "main".to_string(),
            uses_workspace: true,
            moderator: None, responder_policy: None, classification: Classification::CHANNEL_DEFAULT, language: None, can_change: false, members: Vec::new(), former_members: Vec::new() };
        let json = serde_json::to_value(ChannelResponse::from(view.clone())).unwrap();
        assert!(json.get("summary").is_none() && json.get("briefing").is_none(), "{json}");
        let described = snapshot::ChannelSnapshot { summary: Some(Summary::new("Planning").unwrap()), briefing: Some(Briefing::new("Dates in UTC.").unwrap()), ..view };
        let json = serde_json::to_value(ChannelResponse::from(described)).unwrap();
        assert_eq!((&json["summary"], &json["briefing"]), (&serde_json::json!("Planning"), &serde_json::json!("Dates in UTC.")));
        let request: DescriptionRequest = serde_json::from_value(serde_json::json!({ "summary": "Planning" })).unwrap();
        assert_eq!((request.summary.as_deref(), request.briefing), (Some("Planning"), None), "one left out is none: removed");
    }

    #[test]
    fn a_stream_lists_its_agents_notes_and_approvals_beside_the_channel() {
        use caelumora_model::agent::AgentId;
        use caelumora_model::note::{Note, NoteId};
        let (alice, helper) = (ActorId::User(UserId::new()), ActorId::Agent(AgentId::new()));
        let note = Note {
            id: NoteId(1),
            text: "noted".to_string(),
            classification: Classification::CHANNEL_DEFAULT,
            origin: Default::default(),
            after: Default::default(),
            written_at: chrono::Utc::now(),
        };
        let member = |id, name: &str, notes: Vec<Note>| snapshot::MemberSnapshot {
            id, name: Some(name.to_string()), handle: None, can_read_context: false, can_read_tools: false, can_read_components: false, can_approve: false, can_retry: false, can_cancel: false, usage: None, notes, pending_approvals: Vec::new(), pending_questions: Vec::new(),
            left: None, can_delete: false, turn_under_way: None, state: None, commands: Vec::new(),
            introduction: None, personality: None, personality_kind: None, owner: None, owner_name: None,
        };
        let channel = snapshot::ChannelSnapshot { summary: None, briefing: None, approval_rules: Vec::new(), workspace_id: None,
            uses_workspace: true,
            id: ChannelId::new(),
            name: "main".to_string(),
            moderator: None,
            responder_policy: None,
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            can_change: false,
            members: vec![member(alice, "alice", Vec::new()), member(helper, "Helper", vec![note])],
            former_members: Vec::new(),
        };

        let json = serde_json::to_value(ChannelSnapshot::of(channel, Vec::new(), false)).unwrap();

        assert_eq!(json["notes"].as_array().map(Vec::len), Some(1), "one entry per agent");
        assert_eq!(json["notes"][0]["name"], "Helper");
        assert_eq!(json["notes"][0]["notes"][0]["id"], 1);
        assert_eq!(json["pending_approvals"], serde_json::json!([]));
        assert!(json["channel"]["members"][1].get("notes").is_none(), "not repeated at the member");
    }

    #[test]
    fn a_former_member_is_answered_apart_with_who_removed_it_and_when() {
        let (alice, helper) = (ActorId::User(UserId::new()), ActorId::Agent(AgentId::new()));
        let left = Removal::now(alice);
        let member = |id, left| snapshot::MemberSnapshot { id, name: Some("x".to_string()), handle: None, can_read_context: true, can_read_tools: false, can_read_components: false, can_approve: false, can_retry: false, can_cancel: false, usage: None, notes: Vec::new(), pending_approvals: Vec::new(), pending_questions: Vec::new(), left, can_delete: false, turn_under_way: None, state: None, commands: Vec::new(), introduction: None, personality: None, personality_kind: None, owner: None, owner_name: None };
        let view = snapshot::ChannelSnapshot { summary: None, briefing: None, approval_rules: Vec::new(), workspace_id: None,
            uses_workspace: true,
            id: ChannelId::new(),
            name: "main".to_string(),
            moderator: None,
            responder_policy: None,
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            can_change: true,
            members: vec![member(alice, None)],
            former_members: vec![member(helper, Some(left.clone()))],
        };
        let json = serde_json::to_value(ChannelResponse::from(view)).unwrap();
        assert!(json["members"][0].get("left").is_none(), "a member has not left");
        assert_eq!(json["former_members"][0]["id"], serde_json::to_value(helper).unwrap());
        assert_eq!(json["former_members"][0]["left"]["by"], serde_json::to_value(alice).unwrap());
        assert_eq!(json["former_members"][0]["can_read_context"], true, "what it saw may still be read");
    }

    #[test]
    fn what_a_participant_used_is_answered_only_where_it_was_read() {
        let usage = TokenUsage { calls: 2, prompt_tokens: 300, completion_tokens: 20, context_size: Some(8192), context_tokens: 120 };
        let member = |usage| snapshot::MemberSnapshot { id: ActorId::User(UserId::new()), name: Some("alice".to_string()), handle: None, can_read_context: false, can_read_tools: false, can_read_components: false, can_approve: false, can_retry: false, can_cancel: false, usage, notes: Vec::new(), pending_approvals: Vec::new(), pending_questions: Vec::new(), left: None, can_delete: false, turn_under_way: None, state: None, commands: Vec::new(), introduction: None, personality: None, personality_kind: None, owner: None, owner_name: None };

        let json = serde_json::to_value(ChannelMemberResponse::of(member(Some(usage))).unwrap()).unwrap();
        assert_eq!(json["usage"], serde_json::json!({ "calls": 2, "prompt_tokens": 300, "completion_tokens": 20, "context_size": 8192, "context_tokens": 120 }));
        let json = serde_json::to_value(ChannelMemberResponse::of(member(None)).unwrap()).unwrap();
        assert!(json.get("usage").is_none());
        assert!(json.get("turn_under_way").is_none(), "no answer under way, nothing said of one");
        let turn = TurnId::new();
        let json = serde_json::to_value(ChannelMemberResponse::of(snapshot::MemberSnapshot { turn_under_way: Some(turn), ..member(None) }).unwrap()).unwrap();
        assert_eq!(json["turn_under_way"], serde_json::json!(turn.to_string()), "the turn its messages carry");
        assert!(json.get("state").is_none(), "where nothing runs, no state");
        let json = serde_json::to_value(ChannelMemberResponse::of(snapshot::MemberSnapshot { state: Some(ActorState::WaitingForUser), ..member(None) }).unwrap()).unwrap();
        assert_eq!(json["state"], serde_json::json!("WaitingForUser"), "named as `StateChanged` names it (issue 243)");
        assert!(json.get("commands").is_none(), "absent without commands");
        let mark = Command { name: "mark".to_string(), description: "Marks.".to_string(), usage: None, owners_only: false };
        let json = serde_json::to_value(ChannelMemberResponse::of(snapshot::MemberSnapshot { commands: vec![mark], ..member(None) }).unwrap()).unwrap();
        assert_eq!(json["commands"][0]["name"], "mark", "an agent's commands, as `GET …/commands` gives the channel's (issue 269)");
    }

    #[test]
    fn a_participants_profile_is_answered_only_where_it_was_read() {
        let carol = UserId::new();
        let member = snapshot::MemberSnapshot { id: ActorId::Agent(AgentId::new()), name: Some("Helper".to_string()), handle: None, can_read_context: false, can_read_tools: false, can_read_components: false, can_approve: false, can_retry: false, can_cancel: false, usage: None, notes: Vec::new(), pending_approvals: Vec::new(), pending_questions: Vec::new(), left: None, can_delete: false, turn_under_way: None, state: None, commands: Vec::new(), introduction: None, personality: None, personality_kind: None, owner: None, owner_name: None };

        let json = serde_json::to_value(ChannelMemberResponse::of(member.clone()).unwrap()).unwrap();
        for field in ["introduction", "personality", "personality_kind", "owner", "owner_name"] {
            assert!(json.get(field).is_none(), "{field}: not read, not said");
        }
        let profiled = snapshot::MemberSnapshot {
            introduction: Some("Answers questions.".to_string()), personality: Some("plain".to_string()), personality_kind: Some(PersonalityKind::External),
            owner: Some(ActorId::User(carol)), owner_name: Some("carol".to_string()), ..member
        };
        let json = serde_json::to_value(ChannelMemberResponse::of(profiled).unwrap()).unwrap();
        assert_eq!(json["introduction"], "Answers questions.");
        assert_eq!((&json["personality"], &json["personality_kind"]), (&serde_json::json!("plain"), &serde_json::json!("external")));
        assert_eq!(json["owner"], serde_json::to_value(ActorId::User(carol)).unwrap(), "as every actor is named");
        assert_eq!(json["owner_name"], "carol");
    }

}
