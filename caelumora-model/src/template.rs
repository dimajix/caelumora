//! Session templates: what a session is made of - its participants and
//! channels - before there is one. The model builds a session from a
//! template and the participants it names; finding or creating those
//! participants in the store is the runtime's (`SessionManager::
//! create_from_template`, issue 88), so nothing here touches a store.

use crate::agent::PersonalityId;
use crate::approval::{ApprovalRule, ApprovalStrategy, Approver, ApproverId};
use crate::channel::{Channel, Participant};
use crate::component::ComponentConfig;
use crate::confidentiality::Classification;
use crate::value::language::Language;
use crate::scope::Scope::{GlobalScope, SessionScope, UserScope};
use crate::value::name::{Name, ScopedName};
use crate::value::text::{Briefing, Description, Summary};
use crate::removal::{Deletable, NoEntity, Removal, Stored};
use crate::scope::{Scope, ScopeField, Scoped};
use crate::privacy::PrivacyConfig;
use crate::session::{Session, SessionId};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::actor::ActorId;
use crate::security::{AclEntry, Secured, SecurityAttributes};
use crate::user::UserId;
use crate::store::EntityKind;

/// A template that cannot be made into a session. The messages are meant
/// for users (see `caelumora_runtime::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// A channel names a participant the template does not have.
    #[error("channel participant '{0}' is not a participant of the template")]
    ParticipantNotFound(String),
    /// A user participant other than "user", which only the session's owner
    /// fills.
    #[error("the template's user '{0}' is filled by nobody: only \"user\", the session's owner, is")]
    UnboundUser(String),
    /// A scope that cannot be made yet.
    #[error("the scope '{0}' is not supported yet")]
    UnsupportedScope(&'static str),
    /// The agent the template names exists in the scope it asks for, but
    /// with another personality (issue 178): neither reused nor replaced.
    #[error("the agent '{0}' already exists where the template asks for it, with another personality")]
    AgentNameTaken(Name),
    /// The personality of an agent lies in a session's or a channel's
    /// scope the agent would not lie in (issue 178).
    #[error("the personality '{0}' cannot make an agent outside the session or channel it lies in")]
    PersonalityOutOfScope(Name),
    /// The workspace the template names exists in the scope it asks for,
    /// but in another storage (issue 251): neither reused nor replaced.
    #[error("the workspace '{0}' already exists where the template asks for it, in another storage")]
    WorkspaceNameTaken(Name),
}

pub type TemplateResult<T> = Result<T, TemplateError>;

/// The name of the user participant that the session's owner fills.
pub const OWNER: &str = "user";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScopeTemplate {
    SessionScope,
    UserScope,
    GroupScope,
    GlobalScope
}

impl ScopeTemplate {
    /// The scope of an agent of the template, in the session `session_id`
    /// of `owner`.
    ///
    /// # Errors
    /// `TemplateError::UnsupportedScope` for a group scope: there are no
    /// groups yet.
    pub fn instantiate(&self, session_id: SessionId, owner: UserId) -> TemplateResult<Scope> {
        match self {
            ScopeTemplate::SessionScope => Ok(SessionScope { session_id }),
            ScopeTemplate::UserScope => Ok(UserScope { user_id: owner }),
            ScopeTemplate::GroupScope => Err(TemplateError::UnsupportedScope("group")),
            ScopeTemplate::GlobalScope => Ok(GlobalScope),
        }
    }
}

/// A channel of a template. Templates are written by hand (seed files):
/// the fields with a default may be left out (issue 108).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelTemplate {
    pub name: Name,
    /// What the channel is for, in a line, and what its agents should know
    /// (issue 245): copied into it, where they are changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// The language the channel is held in (issue 56), whoever creates it;
    /// none in particular unless named - a frontend may then set the one its
    /// creator prefers (issue 331).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// The channel's components (issue 174): its moderator and the policy
    /// that fills its slot; the defaults if none are named.
    #[serde(default)]
    pub components: ComponentConfig,
    /// The classification of the channel's contents (issue 133);
    /// `internal` without compartments unless named.
    #[serde(default)]
    pub classification: Classification,
    /// The session template's participants that take part, by their name in
    /// the template. A channel that names no approver gets one anyway
    /// (`Channel::build`).
    pub participants: Vec<String>,
    /// What the channel restricts of tool calls (issue 13): rules that ask
    /// or deny. None unless named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_rules: Vec<ApprovalRule>,
    /// Whether the channel uses the session's workspace (issue 251): it
    /// does unless named otherwise.
    #[serde(default = "crate::channel::uses_workspace_by_default")]
    pub uses_workspace: bool,
    /// What the channel grants besides what every channel does: entries of
    /// its access list, such as whom it lets join (issue 221). None unless
    /// named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<AclEntry>,
}

impl ChannelTemplate {
    /// Builds the channel of session `session_id`, owned by `owner`, with
    /// the entries of `access` besides those every channel has.
    /// `session_participants` pairs each participant of the session with
    /// its name in the session template, which is what `self.participants`
    /// refers to - not the user's or agent's own name: the template's "user"
    /// is whoever the session was created for. Held in the template's
    /// language, if it names one (issue 56).
    pub fn instantiate(&self, session_id: SessionId, owner: UserId, session_participants: &[(String, Participant)]) -> TemplateResult<Channel> {
        let participants = self.participants.iter()
            .map(|name| session_participants.iter()
                .find(|(template_name, _)| template_name == name)
                .map(|(_, participant)| participant.clone())
                .ok_or_else(|| TemplateError::ParticipantNotFound(name.clone())))
            .collect::<TemplateResult<Vec<_>>>()?;
        let channel = Channel::build(session_id, ActorId::User(owner), self.name.clone(), self.components.clone(), self.classification.clone(), self.language.clone(), participants);
        let mut security_attributes = channel.security_attributes.clone();
        security_attributes.acl.extend(self.access.iter().copied());
        Ok(Channel { approval_rules: self.approval_rules.clone(), summary: self.summary.clone(), briefing: self.briefing.clone(), uses_workspace: self.uses_workspace, security_attributes, ..channel })
    }
}

define_id!(SessionTemplateId);

/// What sessions are made from. Written by hand (seed files): the fields
/// with a default may be left out (issue 108).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTemplate {
    pub id: SessionTemplateId,
    /// What it is found by: its name and where it lies (issue 177), the
    /// global scope unless named.
    #[serde(flatten)]
    pub name: ScopedName,
    /// What sessions made from it are for: a catalogue line, offered to
    /// whoever starts one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// What sessions made from it are for, in a line, and what their agents
    /// should know (issue 245): copied into each, where they are changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// Every actor of the session - users, agents and approvers - each with
    /// the name its channels refer to it by.
    pub participants: Vec<ParticipantTemplate>,
    pub channels: Vec<ChannelTemplate>,
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
    /// User commands of sessions made from this template, in addition to the
    /// global ones from `AppConfig` (issue 28) - for commands that belong to
    /// what the session is for, such as an integration it uses.
    #[serde(default)]
    pub commands: ComponentConfig,
    /// The privacy policy of sessions made from this template (issue 134).
    #[serde(default)]
    pub privacy: PrivacyConfig,
    /// What sessions made from this template restrict of tool calls (issue
    /// 13): rules that ask or deny. None unless named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_rules: Vec<ApprovalRule>,
    /// What the sessions made from it grant besides what every session does
    /// (`Session::attributes_of`): entries of their access lists, such as
    /// whom they let join (issue 221) - `{ everyone, join }` for sessions
    /// anybody may join. Each session's owner may change its own; none
    /// unless named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<AclEntry>,
    /// The workspace sessions made from it have (issue 251), found or made
    /// as they are made and attached to them; none unless named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceTemplate>,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

/// The workspace of a template's sessions (issue 251, decision 13): found
/// by its name in `scope` as an agent is - in the owner's scope or the
/// global one, reused by every session that finds it - or made there; in a
/// session's scope, made for every session anew and gone with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceTemplate {
    pub name: Name,
    pub scope: ScopeTemplate,
    /// The storage it is made in, by its configured name.
    pub storage: String,
    /// How it is classified when it is made; `internal` without
    /// compartments unless named. One found keeps its own.
    #[serde(default)]
    pub classification: Classification,
    /// What it holds, when it is made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
}

impl WorkspaceTemplate {
    /// Its name where it lies for the session `session_id` of `owner`.
    ///
    /// # Errors
    /// `TemplateError::UnsupportedScope` for a group scope.
    pub fn scoped_name(&self, session_id: SessionId, owner: UserId) -> TemplateResult<ScopedName> {
        Ok(ScopedName { name: self.name.clone(), scope: self.scope.instantiate(session_id, owner)? })
    }
}

/// A template lies in a user's scope or everywhere (issue 178): one of a
/// session's or a channel's scope would make sessions that can never lie
/// in it, so it is not stored.
impl Stored for SessionTemplate {
    const KIND: EntityKind = EntityKind::new("session_template");
    type Id = SessionTemplateId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> SessionTemplateId {
        self.id
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(Scoped::scoped_name(self))
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn validate(&self) -> Result<(), String> {
        match self.name.scope {
            Scope::SessionScope { .. } | Scope::ChannelScope { .. } =>
                Err(format!("the session template '{}' cannot lie in a session's or a channel's scope", self.name)),
            Scope::UserScope { .. } | Scope::GroupScope { .. } | Scope::GlobalScope => Ok(()),
        }
    }
}

impl Deletable for SessionTemplate {
    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

impl Secured for SessionTemplate {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for SessionTemplate {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

impl SessionTemplate {

    /// Builds the session `id` of `owner`, named `name`, and its channels -
    /// documents of their own, which the session lists by id (issue 70).
    /// `participants` are the template's participants as found or created
    /// for it (`ParticipantTemplate::request`), each with its name in the
    /// template. Nothing is stored: that is the caller's, so that it can
    /// announce the session where it is created.
    pub fn build(&self, id: SessionId, owner: UserId, name: Name, participants: Vec<(String, Participant)>) -> TemplateResult<(Session, Vec<Channel>)> {
        let channels = self.channels.iter()
            .map(|channel| channel.instantiate(id, owner, &participants))
            .collect::<TemplateResult<Vec<_>>>()?;
        let participants: Vec<Participant> = participants.into_iter().map(|(_, participant)| participant).collect();
        let security_attributes = Session::attributes_of(id, owner, &self.access);
        let now = Utc::now();
        let session = Session {
            id,
            name,
            summary: self.summary.clone(),
            briefing: self.briefing.clone(),
            participants,
            channels: channels.iter().map(|c: &Channel| c.id).collect(),
            commands: self.commands.clone(),
            privacy: self.privacy,
            approval_rules: self.approval_rules.clone(),
            workspace: None,
            created_at: now,
            updated_at: now,
            security_attributes,
            deleted: None,
        };
        Ok((session, channels))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantTemplate {
    /// A user; "user" is the session's owner.
    User { name: String },
    /// An agent of `personality_id`, found by its name in `scope` or made.
    Agent {
        name: Name,
        scope: ScopeTemplate,
        personality_id: PersonalityId,
    },
    /// An approver, which decides tool calls that need approval by
    /// `strategy`; it has no record of its own and is made with the session.
    Approver {
        name: String,
        #[serde(default = "default_strategy")]
        strategy: ApprovalStrategy,
    },
}

fn default_strategy() -> ApprovalStrategy {
    ApprovalStrategy::Any
}

/// What a participant of a template is for one session: the runtime finds
/// or makes what it names (`SessionManager::create_from_template`).
#[derive(Debug, Clone)]
pub enum ParticipantRequest {
    /// This user, who must exist.
    User(UserId),
    /// The agent of that name in exactly that scope, if it is of
    /// `personality_id`, or a new one of it there.
    Agent { name: ScopedName, personality_id: PersonalityId },
    /// This approver, which needs nothing from the store.
    Approver(Approver),
}

impl ParticipantTemplate {
    /// The name channels use to refer to this participant.
    pub fn name(&self) -> &str {
        match self {
            ParticipantTemplate::User { name } | ParticipantTemplate::Approver { name, .. } => name,
            ParticipantTemplate::Agent { name, .. } => name.as_str(),
        }
    }

    /// What this participant is in the session `session_id` of `owner`.
    ///
    /// # Errors
    /// `TemplateError::UnboundUser` for a user other than "user";
    /// `UnsupportedScope` for an agent of a scope that cannot be made.
    pub fn request(&self, session_id: SessionId, owner: UserId) -> TemplateResult<ParticipantRequest> {
        match self {
            ParticipantTemplate::User { name } if name == OWNER => Ok(ParticipantRequest::User(owner)),
            ParticipantTemplate::User { name } => Err(TemplateError::UnboundUser(name.clone())),
            ParticipantTemplate::Agent { name, scope, personality_id } => Ok(ParticipantRequest::Agent {
                name: ScopedName { name: name.clone(), scope: scope.instantiate(session_id, owner)? },
                personality_id: *personality_id,
            }),
            ParticipantTemplate::Approver { strategy, .. } => Ok(ParticipantRequest::Approver(Approver {
                id: ApproverId(Uuid::new_v4()),
                strategy: *strategy,
            })),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;

    fn template() -> SessionTemplate {
        SessionTemplate {
            access: Vec::new(),
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            security_attributes: Default::default(),
            id: SessionTemplateId(Uuid::new_v4()),
            name: ScopedName::new("pairing", Scope::GlobalScope).unwrap(),
            description: None,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Agent { name: Name::new("helper").unwrap(), scope: ScopeTemplate::UserScope, personality_id: PersonalityId::new() },
                ParticipantTemplate::Approver { name: "approval".to_string(), strategy: ApprovalStrategy::All },
            ],
            channels: vec![
                ChannelTemplate {
                    access: Vec::new(),
                    summary: None, briefing: None, language: None,
                    approval_rules: Vec::new(),
                    uses_workspace: true,
                    name: Name::new("main").unwrap(),
                    components: crate::component::ComponentConfig::default(),
                    classification: Classification::CHANNEL_DEFAULT,
                    participants: vec!["user".to_string(), "helper".to_string(), "approval".to_string()],
                },
                ChannelTemplate {
                    access: Vec::new(),
                    summary: None, briefing: None, language: None,
                    approval_rules: Vec::new(),
                    uses_workspace: true,
                    name: Name::new("side").unwrap(),
                    components: crate::component::ComponentConfig::default(),
                    classification: Classification::CHANNEL_DEFAULT,
                    participants: vec!["user".to_string()],
                },
            ],
            commands: crate::command::default_commands(),
            privacy: Default::default(),
            deleted: None,
        }
    }

    /// The participants as the runtime would resolve the requests.
    fn resolved(template: &SessionTemplate, session: SessionId, owner: UserId) -> Vec<(String, Participant)> {
        template.participants.iter().map(|p| {
            let participant = match p.request(session, owner).unwrap() {
                ParticipantRequest::User(id) => Participant::User(id),
                ParticipantRequest::Agent { .. } => Participant::Agent(AgentId::new()),
                ParticipantRequest::Approver(approver) => Participant::Approver(approver),
            };
            (p.name().to_string(), participant)
        }).collect()
    }

    #[test]
    fn a_session_and_its_channels_are_described_as_their_templates_say() {
        let mut template = template();
        template.summary = Some(Summary::new("Pair programming").unwrap());
        template.briefing = Some(Briefing::new("Review before you merge.").unwrap());
        template.channels[0].summary = Some(Summary::new("Where the pairing happens").unwrap());
        let (id, owner) = (SessionId::new(), UserId::new());
        let (session, channels) = template.build(id, owner, Name::new("s").unwrap(), resolved(&template, id, owner)).unwrap();
        assert_eq!((session.summary.as_ref().map(Summary::as_str), session.briefing.as_ref().map(Briefing::as_str)), (Some("Pair programming"), Some("Review before you merge.")), "copied");
        assert_eq!((channels[0].summary.as_ref().map(Summary::as_str), channels[0].briefing.as_ref()), (Some("Where the pairing happens"), None));
        assert_eq!(channels[1].summary, None, "a channel that says nothing");

        let mut written = serde_json::to_value(&template).unwrap();
        written["channels"][1]["summary"] = serde_json::json!("two\nlines");
        let refused = serde_json::from_value::<SessionTemplate>(written).unwrap_err();
        assert!(refused.to_string().contains("the summary must be one line"), "a template written by hand is checked when it is read: {refused}");
    }

    #[test]
    fn a_channel_is_held_in_the_language_its_template_names() {
        let mut template = template();
        template.channels[0].language = Some(Language::new("de").unwrap());
        let (id, owner) = (SessionId::new(), UserId::new());
        let (_, channels) = template.build(id, owner, Name::new("s").unwrap(), resolved(&template, id, owner)).unwrap();
        assert_eq!(channels[0].language.as_ref().map(Language::as_str), Some("de"), "the template's");
        assert_eq!(channels[1].language, None, "a template that names none: none in particular (issue 331)");

        let named: ChannelTemplate = serde_json::from_value(serde_json::json!({"name": "main", "language": "de", "participants": []})).unwrap();
        assert_eq!(named.language, template.channels[0].language, "a seed names it by its tag");
        assert!(serde_json::from_value::<ChannelTemplate>(serde_json::json!({"name": "main", "language": "x", "participants": []})).is_err(), "checked by its form");
    }

    #[test]
    fn the_requests_name_the_owner_and_the_agents_scope() {
        let template = template();
        let (session, owner) = (SessionId::new(), UserId::new());
        let requests: Vec<_> = template.participants.iter().map(|p| p.request(session, owner).unwrap()).collect();

        assert!(matches!(requests[0], ParticipantRequest::User(id) if id == owner), "\"user\" is the owner");
        assert!(matches!(&requests[1], ParticipantRequest::Agent { name: ScopedName { name, scope: Scope::UserScope { user_id } }, .. } if *name == "helper" && *user_id == owner));
        assert!(matches!(&requests[2], ParticipantRequest::Approver(a) if a.strategy == ApprovalStrategy::All), "the template's strategy");

        let other = ParticipantTemplate::User { name: "guest".to_string() };
        assert!(matches!(other.request(session, owner), Err(TemplateError::UnboundUser(_))), "only the owner fills a user");
        let group = ParticipantTemplate::Agent { name: Name::new("x").unwrap(), scope: ScopeTemplate::GroupScope, personality_id: PersonalityId::new() };
        assert!(matches!(group.request(session, owner), Err(TemplateError::UnsupportedScope("group"))), "an error, not a panic");
    }

    #[test]
    fn channels_refer_to_participants_by_their_template_name() {
        let template = template();
        let (id, owner) = (SessionId::new(), UserId::new());

        let (session, channels) = template.build(id, owner, Name::new("s").unwrap(), resolved(&template, id, owner)).unwrap();
        assert_eq!(session.channels, channels.iter().map(|c| c.id).collect::<Vec<_>>(), "the session lists its channels by id");
        assert_eq!(session.participants.len(), 3, "every actor of the template");

        let approvers = |c: &Channel| c.participants.iter().filter(|p| matches!(p, Participant::Approver(_))).count();
        assert_eq!(channels[0].participants.len(), 3);
        assert_eq!(approvers(&channels[0]), 1, "the template's approver, not a second one");
        assert_eq!(approvers(&channels[1]), 1, "a channel that names none gets one");
        assert!(matches!(channels[1].participants[0], Participant::User(u) if u == owner), "the template's \"user\" is the owner");
        assert_eq!(session.commands.component_nodes.len(), crate::command::default_commands().component_nodes.len(), "the session keeps its template's commands");
        assert_eq!(channels[0].classification, Classification::CHANNEL_DEFAULT);
        assert_eq!(session.privacy, PrivacyConfig::Private, "private unless the template says otherwise");
        let shared = SessionTemplate { privacy: PrivacyConfig::SharedAudience, ..template.clone() };
        let (session, _) = shared.build(id, owner, Name::new("s").unwrap(), resolved(&shared, id, owner)).unwrap();
        assert_eq!(session.privacy, PrivacyConfig::SharedAudience, "the template's policy");

        // A channel naming someone the template does not have.
        let mut stray = template.clone();
        stray.channels[1].participants.push("ghost".to_string());
        assert!(matches!(stray.build(id, owner, Name::new("x").unwrap(), resolved(&stray, id, owner)), Err(TemplateError::ParticipantNotFound(_))));

        let unnamed: ChannelTemplate = serde_json::from_value(serde_json::json!({"name": "main", "participants": []})).unwrap();
        assert_eq!(unnamed.classification, Classification::general(crate::confidentiality::Confidentiality::Internal), "the default");
        let approver: ParticipantTemplate = serde_json::from_value(serde_json::json!({"Approver": {"name": "approval"}})).unwrap();
        assert!(matches!(approver, ParticipantTemplate::Approver { strategy: ApprovalStrategy::Any, .. }), "the default strategy");

    }
}
