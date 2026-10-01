use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{Classification, Compartment};
use caelumora_model::message::Message;
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::template::{SessionTemplate, SessionTemplateId};
use caelumora_model::snapshot;
use caelumora_runtime::view::SessionView;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub template_id: Uuid,
    pub name: Option<String>,
}

/// A session in a list, or as creating or renaming it answers.
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: SessionId,
    pub name: String,
    /// Whom the session lets read what was said in it (issue 134).
    pub privacy: PrivacyConfig,
}

impl From<&caelumora_model::session::Session> for SessionResponse {
    fn from(session: &caelumora_model::session::Session) -> Self {
        Self { id: session.id, name: session.metadata.name.clone(), privacy: session.privacy }
    }
}

impl From<snapshot::SessionSnapshot> for SessionResponse {
    fn from(view: snapshot::SessionSnapshot) -> Self {
        Self { id: view.id, name: view.name, privacy: view.privacy }
    }
}

/// What a participant is called whose user or agent no longer exists.
const UNKNOWN: &str = "(unknown)";

/// A session template, as a client offers it for a new session - its id,
/// name and description, not the template itself (issue 106).
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: SessionTemplateId,
    pub name: String,
    pub description: String,
}

impl From<&SessionTemplate> for TemplateResponse {
    fn from(template: &SessionTemplate) -> Self {
        Self { id: template.id, name: template.metadata.name.clone(), description: template.metadata.description.clone() }
    }
}

/// A compartment a classification may name (issue 133), as configured.
#[derive(Debug, Serialize)]
pub struct CompartmentResponse {
    pub name: String,
}

impl From<&Compartment> for CompartmentResponse {
    fn from(compartment: &Compartment) -> Self {
        Self { name: compartment.name().to_string() }
    }
}

/// What kind of participant an actor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantKind {
    User,
    Agent,
    Approver,
}

impl ParticipantKind {
    /// The kind of participant `id` is; `None` for an id that is none - the
    /// system, nobody.
    pub fn of(id: ActorId) -> Option<Self> {
        match id {
            ActorId::User(_) => Some(Self::User),
            ActorId::Agent(_) => Some(Self::Agent),
            ActorId::Approver(_) => Some(Self::Approver),
            _ => None,
        }
    }
}

/// Whether an event stream starts with a snapshot of the current state
/// (issue 34). Off by default, so a client that does not want one does not
/// pay for it.
#[derive(Debug, Default, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub snapshot: bool,
}

/// The state of a channel when a stream connects: its members and the end of
/// its history. Older messages are fetched over REST - `has_more` says there
/// are some.
#[derive(Debug, Serialize)]
pub struct ChannelSnapshot {
    pub channel: ChannelResponse,
    pub messages: Vec<Message>,
    pub has_more: bool,
    /// The notes every agent of the channel published (issue 17); a change
    /// is announced as `AgentChannelStateChanged` on the event stream, and
    /// the notes reloaded from `GET …/channels/{cid}/notes` (issue 65).
    pub notes: Vec<PublishedNotesResponse>,
}

/// The notes one agent published in a channel.
#[derive(Debug, Serialize)]
pub struct PublishedNotesResponse {
    pub agent_id: ActorId,
    pub name: String,
    pub notes: Vec<caelumora_model::note::Note>,
}

impl From<caelumora_runtime::view::PublishedNotes> for PublishedNotesResponse {
    fn from(published: caelumora_runtime::view::PublishedNotes) -> Self {
        Self { agent_id: published.agent, name: published.name, notes: published.notes }
    }
}

/// The state of the system when a global stream connects: the sessions and
/// channels the user may read, as the REST endpoints decide it.
#[derive(Debug, Serialize)]
pub struct GlobalSnapshot {
    pub sessions: Vec<SessionSnapshot>,
}

/// A session with its channels: an entry of the global snapshot, and what
/// `GET /sessions/{id}` answers.
#[derive(Debug, Serialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub name: String,
    /// Whom the session lets read what was said in it (issue 134).
    pub privacy: PrivacyConfig,
    /// Whether you may change it - rename it, change its privacy.
    pub can_change: bool,
    pub channels: Vec<ChannelResponse>,
    /// Everyone who takes part in the session - whom a new channel can be
    /// made of (issue 09).
    pub participants: Vec<ParticipantResponse>,
}

impl From<snapshot::SessionSnapshot> for SessionSnapshot {
    fn from(view: snapshot::SessionSnapshot) -> Self {
        Self {
            id: view.id,
            name: view.name,
            privacy: view.privacy,
            can_change: view.can_change,
            channels: view.channels.into_iter().map(Into::into).collect(),
            // Every participant is a user, an agent or an approver.
            participants: view.participants.into_iter().filter_map(ParticipantResponse::of).collect(),
        }
    }
}

/// A participant of a session.
#[derive(Debug, Serialize)]
pub struct ParticipantResponse {
    pub id: ActorId,
    pub name: String,
    pub kind: ParticipantKind,
}

impl ParticipantResponse {
    /// `view` as the API shows it; `None` for an id that is no participant.
    fn of(view: snapshot::ParticipantSnapshot) -> Option<Self> {
        Some(Self { id: view.id, kind: ParticipantKind::of(view.id)?, name: view.name.unwrap_or_else(|| UNKNOWN.to_string()) })
    }
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionRequest {
    pub name: String,
}

/// A session's new privacy policy (issue 134).
#[derive(Debug, Deserialize)]
pub struct SessionPrivacyRequest {
    pub privacy: PrivacyConfig,
}

/// A participant of the session to add to a channel (issue 46), with how
/// much of the channel's history they may see - all of it unless asked.
#[derive(Debug, Deserialize)]
pub struct AddParticipantRequest {
    pub participant: ActorId,
    #[serde(default)]
    pub history: caelumora_model::channel::HistoryAccess,
}

/// A channel to create in a running session (issue 09).
#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
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
}

#[derive(Debug, Serialize)]
pub struct ChannelResponse {
    pub id: ChannelId,
    pub name: String,
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
    /// Whether the requesting user may rename the channel and add or remove
    /// its participants (issue 46).
    pub can_change: bool,
}

impl From<snapshot::ChannelSnapshot> for ChannelResponse {
    fn from(view: snapshot::ChannelSnapshot) -> Self {
        Self {
            id: view.id,
            name: view.name,
            // Every member is a user, an agent or an approver.
            members: view.members.into_iter().filter_map(ChannelMemberResponse::of).collect(),
            moderator: view.moderator,
            responder_policy: view.responder_policy,
            classification: view.classification,
            can_change: view.can_change,
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
}

impl ChannelMemberResponse {
    /// `view` as the API shows it; `None` for an id that is no participant.
    fn of(view: snapshot::MemberSnapshot) -> Option<Self> {
        Some(Self {
            id: view.id,
            kind: ParticipantKind::of(view.id)?,
            name: view.name.unwrap_or_else(|| UNKNOWN.to_string()),
            handle: view.handle,
            can_read_context: view.can_read_context,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    /// How the message is classified (issue 133); the channel's if absent.
    /// Another than the channel's releases it: to a wider audience or a
    /// lower level, as long as it may flow into the channel.
    pub classification: Option<Classification>,
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
#[derive(Debug, Serialize)]
pub struct CommandReplyResponse {
    pub command: String,
    pub output: Option<String>,
}

/// A turn an agent gave up (issue 145): why and when, and the messages it
/// had produced, which stay in the agent's transcript as a branch no longer
/// active. What each of them was produced from is its context, as for any
/// message of the agent.
#[derive(Debug, Serialize)]
pub struct GivenUpTurnResponse {
    pub reason: caelumora_model::transcript::GivenUpReason,
    pub at: chrono::DateTime<chrono::Utc>,
    pub messages: Vec<Message>,
}

impl From<caelumora_model::transcript::GivenUpTurn> for GivenUpTurnResponse {
    fn from(turn: caelumora_model::transcript::GivenUpTurn) -> Self {
        Self { reason: turn.given_up.reason, at: turn.given_up.at, messages: turn.messages }
    }
}

/// What an agent's model saw when it produced a message (issues 06, 47):
/// the messages of its transcript and the prompt parts of that turn, in the
/// order the model was given them, with the digest of the context, and the
/// tools it was offered (issue 137).
#[derive(Debug, Serialize)]
pub struct MessageContextResponse {
    pub message_id: caelumora_model::message::MessageId,
    pub actor_id: ActorId,
    pub transcript_digest: Option<caelumora_model::digest::Digest>,
    pub items: Vec<SeenItemResponse>,
    /// What was read from notes and histories before the message was
    /// produced (issue 17).
    pub knowledge: Vec<KnowledgeRefResponse>,
    /// The digest of the set of tools the model was offered, `None` if it
    /// was offered none (issue 137).
    pub tools_digest: Option<caelumora_model::digest::Digest>,
    /// The tools, as the model was told of them.
    pub tools: Vec<caelumora_model::tool::ToolDefinition>,
}

/// Which of the classifications in a message's context set its label
/// (issue 170). The label of a message is the join of everything its turn
/// had seen, at most the channel's classification (issue 168); what set it
/// is derived from that, nothing about it is stored:
///
/// - what was not classified counted as the channel, and so set the label
///   to the channel's;
/// - if the classified parts had no audience in common, the label is the
///   channel's, set by the parts with an audience;
/// - otherwise the parts at the label's level set it.
#[derive(Debug)]
pub struct LabelCauses {
    label: Classification,
    disjoint: bool,
}

impl LabelCauses {
    /// The causes of `label`, the classification of a message whose
    /// context's parts are classified `classifications` - those known.
    pub fn new<'a>(label: Classification, classifications: impl IntoIterator<Item = &'a Classification>) -> Self {
        let disjoint = Classification::join_all(classifications).is_none();
        Self { label, disjoint }
    }

    /// Whether a part classified `classification` - `None`: not known -
    /// set the label.
    pub fn set_by(&self, classification: Option<&Classification>) -> bool {
        match classification {
            None => true,
            Some(c) if self.disjoint => !c.compartments().is_empty(),
            Some(c) => c.level() == self.label.level(),
        }
    }

    /// Whether `message` set the label, by its own classification or that
    /// of a tool result in it.
    pub fn set_by_message(&self, message: &Message) -> bool {
        self.set_by(Some(&message.classification)) || message.tool_results.iter().any(|r| self.set_by(Some(&r.classification)))
    }
}

/// Knowledge a message was produced with, with the names of what it names
/// where they are still known.
#[derive(Debug, Serialize)]
pub struct KnowledgeRefResponse {
    /// The session of the channel: another one's, for what an agent
    /// recalled from its memory (issue 134).
    pub session_id: SessionId,
    pub channel_id: caelumora_model::session::ChannelId,
    /// `None` for a channel that is gone, or of another session.
    pub channel_name: Option<String>,
    /// `"notes"`, `"private_notes"`, `"memory"` or `"history"`.
    pub source: &'static str,
    /// Whose notes, for `"notes"` and `"private_notes"`.
    pub agent_id: Option<ActorId>,
    pub agent_name: Option<String>,
    pub digest: Option<caelumora_model::digest::Digest>,
    /// For `"history"`: where the reader's view began, if they joined later
    /// with a limit (issue 72) - `{"from": id}` or `{"after": id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_start: Option<caelumora_model::channel::HistoryStart>,
    /// How what was read was classified when it was read, `null` if that
    /// is not known (issue 168).
    pub classification: Option<Classification>,
    /// Whether it set the label of the message (issue 170).
    pub sets_label: bool,
}

impl KnowledgeRefResponse {
    /// `knowledge` with the names of its channel and agent, as far as
    /// `session` still has them, and whether it set the label of the
    /// message, which `causes` tells.
    pub async fn describe(session: &SessionView, knowledge: &KnowledgeRef, causes: &LabelCauses) -> Self {
        // Named only if the viewer may read it - of this session or another
        // (issue 134) - and it is still there; by id alone otherwise.
        let channel = session.views().channel_of(knowledge.channel).await.unwrap_or_else(|e| {
            tracing::warn!("Cannot reach channel {} of the knowledge read: {:#}", knowledge.channel.channel_id, e);
            None
        });
        let (source, agent_id, digest, history_start) = match knowledge.source {
            KnowledgeSource::Notes { agent, digest } => ("notes", Some(ActorId::Agent(agent)), digest, None),
            KnowledgeSource::PrivateNotes { agent, digest } => ("private_notes", Some(ActorId::Agent(agent)), digest, None),
            KnowledgeSource::History { digest, start } => ("history", None, digest, start),
            KnowledgeSource::Memory { agent, digest } => ("memory", Some(ActorId::Agent(agent)), digest, None),
        };
        let agent_name = match (agent_id, &channel) {
            (Some(id), Some(channel)) => channel.member(id).await.ok().and_then(|m| m.name().map(str::to_string)),
            _ => None,
        };
        Self {
            session_id: knowledge.channel.session_id,
            channel_id: knowledge.channel.channel_id,
            channel_name: channel.map(|c| c.name().to_string()),
            source,
            agent_id,
            agent_name,
            digest,
            history_start,
            classification: knowledge.classification.clone(),
            sets_label: causes.set_by(knowledge.classification.as_ref()),
        }
    }
}

/// One element of a `MessageContextResponse`, with whether it set the
/// label of the message (issue 170).
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeenItemResponse {
    /// A prompt part - the system prompt, notes, ... - the role it was
    /// given as, and how it was classified, `null` if that is not known.
    Prompt {
        digest: caelumora_model::digest::Digest,
        role: caelumora_model::message::MessageRole,
        text: String,
        classification: Option<Classification>,
        sets_label: bool,
    },
    Message { message: Message, sets_label: bool },
}

impl SeenItemResponse {
    /// `item`, with whether it set the label, which `causes` tells.
    pub fn of(item: caelumora_model::context::ContextItem, causes: &LabelCauses) -> Self {
        match item {
            caelumora_model::context::ContextItem::Prompt { digest, role, text, classification } => {
                let sets_label = causes.set_by(classification.as_ref());
                SeenItemResponse::Prompt { digest, role, text, classification, sets_label }
            }
            caelumora_model::context::ContextItem::Conversation { message } => {
                let sets_label = causes.set_by_message(&message);
                SeenItemResponse::Message { message, sets_label }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::approval::ApproverId;
    use caelumora_model::user::UserId;

    #[test]
    fn what_set_a_label_is_told_by_level_by_audience_and_by_what_is_not_known() {
        use caelumora_model::confidentiality::Confidentiality::{Confidential, Internal};
        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let (internal, confidential) = (Classification::general(Internal), Classification::general(Confidential));

        let causes = LabelCauses::new(confidential.clone(), [&internal, &confidential]);
        assert!(causes.set_by(Some(&confidential)), "a part at the label's level set it");
        assert!(!causes.set_by(Some(&internal)), "one below did not");
        assert!(causes.set_by(None), "what is not known counted as the channel");

        let sales = classified(Internal, &["sales"]);
        let causes = LabelCauses::new(confidential.clone(), [&sales, &classified(Internal, &["hr"])]);
        assert!(causes.set_by(Some(&sales)), "without an audience in common, the parts with one set the channel's label");
        assert!(!causes.set_by(Some(&internal)), "a general part did not");

        let causes = LabelCauses::new(confidential.clone(), [&internal, &confidential]);
        let result = caelumora_model::tool::ToolCallResult::of(
            &caelumora_model::tool::ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default() },
            Default::default(), "secret".to_string(), confidential.clone());
        let message = Message { tool_results: vec![result], ..Message::new(internal.clone()) };
        assert!(causes.set_by_message(&message), "a tool result in a message sets the label as the message");
        assert!(!causes.set_by_message(&Message::new(internal)));
    }

    #[test]
    fn a_turn_given_up_is_answered_with_its_reason_and_messages() {
        use caelumora_model::transcript::{GivenUp, GivenUpReason, GivenUpTurn};
        let turn = GivenUpTurn {
            given_up: GivenUp { reason: GivenUpReason::Failed { message: "boom".to_string() }, at: chrono::Utc::now(), turn_entries: 1 },
            messages: vec![Message::new(Classification::CHANNEL_DEFAULT)],
        };
        let json = serde_json::to_value(GivenUpTurnResponse::from(turn)).unwrap();
        assert_eq!(json["reason"], serde_json::json!({ "kind": "failed", "message": "boom" }));
        assert_eq!(json["messages"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_channel_view_is_answered_with_kinds_and_a_name_for_the_deleted() {
        let (alice, approver) = (ActorId::User(UserId::new()), ActorId::Approver(ApproverId(Uuid::new_v4())));
        let member = |id, name: Option<&str>| snapshot::MemberSnapshot { id, name: name.map(str::to_string), handle: None, can_read_context: false };
        let view = snapshot::ChannelSnapshot {
            id: ChannelId::new(),
            name: "main".to_string(),
            moderator: Some("policy_moderator".to_string()),
            responder_policy: Some("broadcast_policy".to_string()),
            classification: Classification::CHANNEL_DEFAULT,
            can_change: true,
            members: vec![member(alice, None), member(approver, Some("Approver")), member(ActorId::System, Some("system"))],
        };

        let response = ChannelResponse::from(view);

        assert!(response.can_change);
        let kinds: Vec<_> = response.members.iter().map(|m| (m.kind, m.name.as_str())).collect();
        assert_eq!(kinds, [(ParticipantKind::User, UNKNOWN), (ParticipantKind::Approver, "Approver")], "the system is no participant");
    }
}
