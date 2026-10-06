use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{Classification, Compartment};
use caelumora_model::message::{Message, TokenUsage};
use caelumora_model::language::Language;
use caelumora_model::name::Name;
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::template::{SessionTemplate, SessionTemplateId};
use caelumora_model::user::{UserProfile, MAX_INTRODUCTION_CHARS};
use caelumora_model::work::TurnId;
use caelumora_model::snapshot;
use caelumora_runtime::view::{Reached, SessionView};
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
        Self { id: session.id, name: session.name.clone(), privacy: session.privacy }
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
        Self { id: template.id, name: template.name.to_string(), description: template.description.clone() }
    }
}

/// The logged-in user's profile (issue 176): how they introduce themselves
/// to the agents of every channel they take part in, and how long that may
/// be.
#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub introduction: Option<String>,
    pub max_introduction_chars: usize,
    /// The language the user prefers (issue 56), a BCP 47 tag such as `de`.
    pub preferred_language: Option<Language>,
}

impl ProfileResponse {
    /// `profile` as answered; one without anything in it if there is none.
    pub(crate) fn of(profile: Option<UserProfile>) -> Self {
        let (introduction, preferred_language) = profile.map_or((None, None), |profile| (profile.introduction, profile.preferred_language));
        Self { introduction, max_introduction_chars: MAX_INTRODUCTION_CHARS, preferred_language }
    }
}

/// The logged-in user's profile as it is to be: their introduction, `null`
/// or blank for none, and the language they prefer, `null` for none - both
/// replaced.
#[derive(Debug, Deserialize)]
pub struct ProfileRequest {
    #[serde(default)]
    pub introduction: Option<String>,
    /// Checked as it is read: a text that is no language tag is a malformed
    /// request.
    #[serde(default)]
    pub preferred_language: Option<Language>,
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

/// An agent the requesting user may read (issue 164), with whether they
/// may delete it.
#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub id: caelumora_model::agent::AgentId,
    pub name: String,
    pub can_delete: bool,
}

impl From<snapshot::AgentSnapshot> for AgentResponse {
    fn from(view: snapshot::AgentSnapshot) -> Self {
        Self { id: view.id, name: view.name, can_delete: view.can_delete }
    }
}

/// Whether a listing lists what is there or what was deleted - the trash
/// (issue 164).
#[derive(Debug, Default, Deserialize)]
pub struct ListOptions {
    #[serde(default)]
    pub deleted: bool,
}

/// Something deleted - a session, a channel - in the trash (issue 164): who
/// deleted it and when, and whether the requesting user may restore it or
/// remove it for good.
#[derive(Debug, Serialize)]
pub struct DeletedResponse<I> {
    pub id: I,
    pub name: String,
    pub deleted: caelumora_model::removal::Removal,
    pub can_restore: bool,
    pub can_purge: bool,
}

impl<I> From<snapshot::DeletedSnapshot<I>> for DeletedResponse<I> {
    fn from(view: snapshot::DeletedSnapshot<I>) -> Self {
        Self { id: view.id, name: view.name, deleted: view.deleted, can_restore: view.can_restore, can_purge: view.can_purge }
    }
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
    /// The approvals the channel's agents wait for (issue 172), as their
    /// turns are stored: the stream tells only of requests made while it
    /// listens. Empty for a viewer who does not take part as a user.
    pub pending_approvals: Vec<caelumora_model::approval::ApprovalRequest>,
}

impl ChannelSnapshot {
    /// The snapshot of a channel's stream: `channel`, whose agent members
    /// carry their notes and pending approvals - listed beside the channel,
    /// as the API has them - and the end of its history.
    pub fn of(mut channel: snapshot::ChannelSnapshot, messages: Vec<Message>, has_more: bool) -> Self {
        let mut notes = Vec::new();
        let mut pending_approvals = Vec::new();
        for member in &mut channel.members {
            if matches!(member.id, ActorId::Agent(_)) {
                let name = member.name.clone().unwrap_or_default();
                notes.push(PublishedNotesResponse { agent_id: member.id, name, notes: std::mem::take(&mut member.notes) });
            }
            pending_approvals.append(&mut member.pending_approvals);
        }
        Self { channel: channel.into(), messages, has_more, notes, pending_approvals }
    }
}

/// The notes one agent published in a channel.
#[derive(Debug, Serialize)]
pub struct PublishedNotesResponse {
    pub agent_id: ActorId,
    pub name: String,
    pub notes: Vec<caelumora_model::note::Note>,
}

impl From<snapshot::PublishedNotes> for PublishedNotesResponse {
    fn from(published: snapshot::PublishedNotes) -> Self {
        Self { agent_id: published.agent, name: published.name, notes: published.notes }
    }
}

/// The private notes one agent wrote in one channel of a session (issue
/// 233), with the digest of their chain.
#[derive(Debug, Serialize)]
pub struct PrivateNotesResponse {
    pub channel_id: ChannelId,
    pub channel_name: String,
    pub notes: Vec<caelumora_model::note::Note>,
    pub digest: Option<caelumora_model::digest::Digest>,
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
    /// Checked as it is read: a text that is no name is a malformed request
    /// (issue 177).
    pub name: Name,
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
    /// the one the requesting user prefers if absent.
    #[serde(default)]
    pub language: Option<Language>,
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
    /// The language its conversation is held in (issue 56); `null` for none
    /// in particular.
    pub language: Option<Language>,
    /// Whether the requesting user may rename the channel and add or remove
    /// its participants (issue 46).
    pub can_change: bool,
    /// Whether the requesting user may ask an agent of the channel to
    /// answer again instead of one of its messages (issue 234); whether it
    /// does is the agent's to decide.
    pub can_retry: bool,
    /// Who took part and left (issue 164), in the order they left, each
    /// with `left`: who removed them and when.
    pub former_members: Vec<ChannelMemberResponse>,
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
            language: view.language,
            can_change: view.can_change,
            can_retry: view.can_retry,
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
    /// Whether the requesting user may approve this participant's tool
    /// calls (issue 14) - only ever for agents taking part. Everybody sees
    /// a pending approval; only who may approve is asked.
    pub can_approve: bool,
    /// What the participant's model calls in the channel used (issue 209):
    /// given by the snapshot of the channel's stream and for a participant
    /// asked for alone, not where channels are listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// For a former member (issue 164): who removed it and when.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<caelumora_model::removal::Removal>,
    /// Whether the requesting user may delete this member - only ever an
    /// agent, its owner (issue 164).
    pub can_delete: bool,
    /// The turn of the answer the member is giving now (issue 240): what a
    /// client that connects during an answer shows as under way, as if it
    /// had seen `ResponseStarted`. Absent if none - also for an external
    /// agent, which answers without a turn here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_under_way: Option<TurnId>,
}

impl ChannelMemberResponse {
    /// `view` as the API shows it; `None` for an id that is no participant.
    pub(crate) fn of(view: snapshot::MemberSnapshot) -> Option<Self> {
        Some(Self {
            id: view.id,
            kind: ParticipantKind::of(view.id)?,
            name: view.name.unwrap_or_else(|| UNKNOWN.to_string()),
            handle: view.handle,
            can_read_context: view.can_read_context,
            can_read_tools: view.can_read_tools,
            can_approve: view.can_approve,
            usage: view.usage,
            left: view.left,
            can_delete: view.can_delete,
            turn_under_way: view.turn_under_way,
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

/// Which answer under way of a participant is asked for again (issue
/// 235): its turn, as `ResponseStarted` and its messages name it.
#[derive(Debug, Deserialize)]
pub struct RetryTurnRequest {
    pub turn_id: TurnId,
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
/// message of the agent. `after` is the message of the agent's history the
/// turn branched off after, where it is shown (issue 146); `None` if it
/// began before anything was said.
#[derive(Debug, Serialize)]
pub struct GivenUpTurnResponse {
    pub reason: caelumora_model::transcript::GivenUpReason,
    pub at: chrono::DateTime<chrono::Utc>,
    pub after: Option<caelumora_model::message::MessageId>,
    pub messages: Vec<HistoryMessageResponse>,
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
    /// The digest of the tool sets the model was offered, `None` if it was
    /// offered none (issue 137).
    pub tools_digest: Option<caelumora_model::digest::Digest>,
    /// The tools, as the model was told of them, in their sets (issue 194).
    pub tool_sets: Vec<caelumora_model::tool::ToolSet>,
}

/// The tools an agent has now in a channel, in their tool sets (issue
/// 194), each tool with what the runtime acts on - whether a call needs
/// approval, who sees it, how its results are classified. None if it has
/// none here: it is no agent run here or has left - and also while the
/// channel does not run.
#[derive(Debug, Serialize)]
pub struct AgentToolsResponse {
    pub tool_sets: Vec<caelumora_model::tool::ToolSet>,
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
    /// Whether the channel was deleted (issue 164) - told only to whoever
    /// could read it were it there; for anyone else, it never was.
    pub channel_deleted: bool,
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
        let reached = session.views().channel_of(knowledge.session, knowledge.channel).await.unwrap_or_else(|e| {
            tracing::warn!("Cannot reach channel {} of the knowledge read: {:#}", knowledge.channel, e);
            Reached::Never
        });
        let channel_deleted = matches!(reached, Reached::Gone(_));
        let channel = reached.there();
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
            session_id: knowledge.session,
            channel_id: knowledge.channel,
            channel_name: channel.map(|c| c.name().to_string()),
            channel_deleted,
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
    fn a_turn_given_up_is_answered_with_its_reason_place_and_messages() {
        use caelumora_model::transcript::GivenUpReason;
        let after = caelumora_model::message::MessageId::new();
        let turn = GivenUpTurnResponse {
            reason: GivenUpReason::Failed { message: "boom".to_string() },
            at: chrono::Utc::now(),
            after: Some(after),
            messages: vec![HistoryMessageResponse { message: Message::new(Classification::CHANNEL_DEFAULT), has_context: true, in_transcript: true }],
        };
        let json = serde_json::to_value(turn).unwrap();
        assert_eq!(json["reason"], serde_json::json!({ "kind": "failed", "message": "boom" }));
        assert_eq!(json["after"], serde_json::json!(after));
        assert_eq!(json["messages"][0]["has_context"], true);
    }

    #[test]
    fn a_channel_view_is_answered_with_kinds_and_a_name_for_the_deleted() {
        let (alice, approver) = (ActorId::User(UserId::new()), ActorId::Approver(ApproverId(Uuid::new_v4())));
        let member = |id, name: Option<&str>| snapshot::MemberSnapshot { id, name: name.map(str::to_string), handle: None, can_read_context: false, can_read_tools: false, can_approve: false, usage: None, notes: Vec::new(), pending_approvals: Vec::new(), left: None, can_delete: false, turn_under_way: None };
        let view = snapshot::ChannelSnapshot {
            id: ChannelId::new(),
            name: "main".to_string(),
            moderator: Some("policy_moderator".to_string()),
            responder_policy: Some("broadcast_policy".to_string()),
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            can_change: true,
            can_retry: true,
            members: vec![member(alice, None), member(approver, Some("Approver")), member(ActorId::System, Some("system"))],
            former_members: Vec::new(),
        };

        let response = ChannelResponse::from(view);

        assert!(response.can_change && response.can_retry);
        let kinds: Vec<_> = response.members.iter().map(|m| (m.kind, m.name.as_str())).collect();
        assert_eq!(kinds, [(ParticipantKind::User, UNKNOWN), (ParticipantKind::Approver, "Approver")], "the system is no participant");
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
            id, name: Some(name.to_string()), handle: None, can_read_context: false, can_read_tools: false, can_approve: false, usage: None, notes, pending_approvals: Vec::new(),
            left: None, can_delete: false, turn_under_way: None,
        };
        let channel = snapshot::ChannelSnapshot {
            id: ChannelId::new(),
            name: "main".to_string(),
            moderator: None,
            responder_policy: None,
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            can_change: false,
            can_retry: false,
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
    fn something_deleted_is_answered_with_who_deleted_it_and_what_may_be_done() {
        let alice = ActorId::User(UserId::new());
        let id = SessionId::new();
        let removal = caelumora_model::removal::Removal::now(alice);
        let deleted = snapshot::DeletedSnapshot { id, name: "hers".to_string(), deleted: removal, can_restore: true, can_purge: false };
        let json = serde_json::to_value(DeletedResponse::from(deleted)).unwrap();
        assert_eq!(json["id"], serde_json::to_value(id).unwrap());
        assert_eq!(json["deleted"]["by"], serde_json::to_value(alice).unwrap());
        assert!(json["deleted"]["at"].is_string());
        assert_eq!((json["can_restore"].as_bool(), json["can_purge"].as_bool()), (Some(true), Some(false)));
    }

    #[test]
    fn a_former_member_is_answered_apart_with_who_removed_it_and_when() {
        let (alice, helper) = (ActorId::User(UserId::new()), ActorId::Agent(caelumora_model::agent::AgentId::new()));
        let left = caelumora_model::removal::Removal::now(alice);
        let member = |id, left| snapshot::MemberSnapshot { id, name: Some("x".to_string()), handle: None, can_read_context: true, can_read_tools: false, can_approve: false, usage: None, notes: Vec::new(), pending_approvals: Vec::new(), left, can_delete: false, turn_under_way: None };
        let view = snapshot::ChannelSnapshot {
            id: ChannelId::new(),
            name: "main".to_string(),
            moderator: None,
            responder_policy: None,
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            can_change: true,
            can_retry: true,
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
        let usage = TokenUsage { calls: 2, prompt_tokens: 300, completion_tokens: 20, last_call_tokens: 170, context_size: Some(8192) };
        let member = |usage| snapshot::MemberSnapshot { id: ActorId::User(UserId::new()), name: Some("alice".to_string()), handle: None, can_read_context: false, can_read_tools: false, can_approve: false, usage, notes: Vec::new(), pending_approvals: Vec::new(), left: None, can_delete: false, turn_under_way: None };

        let json = serde_json::to_value(ChannelMemberResponse::of(member(Some(usage))).unwrap()).unwrap();
        assert_eq!(json["usage"], serde_json::json!({ "calls": 2, "prompt_tokens": 300, "completion_tokens": 20, "last_call_tokens": 170, "context_size": 8192 }));
        let json = serde_json::to_value(ChannelMemberResponse::of(member(None)).unwrap()).unwrap();
        assert!(json.get("usage").is_none());
        assert!(json.get("turn_under_way").is_none(), "no answer under way, nothing said of one");
        let turn = caelumora_model::work::TurnId::new();
        let json = serde_json::to_value(ChannelMemberResponse::of(snapshot::MemberSnapshot { turn_under_way: Some(turn), ..member(None) }).unwrap()).unwrap();
        assert_eq!(json["turn_under_way"], serde_json::json!(turn.to_string()), "the turn its messages carry");
    }

    #[test]
    fn a_language_is_a_tag_checked_as_the_request_is_read() {
        let request: ProfileRequest = serde_json::from_str(r#"{"introduction": null, "preferred_language": "de_ch"}"#).unwrap();
        assert_eq!(request.preferred_language, Some(Language::parse("de-CH").unwrap()));
        assert!(serde_json::from_str::<ProfileRequest>(r#"{"preferred_language": "Deutsch"}"#).is_err(), "no tag");
        let request: CreateChannelRequest = serde_json::from_str(r#"{"name": "side"}"#).unwrap();
        assert_eq!(request.language, None, "the preferred one when left out");
    }
}
