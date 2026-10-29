//! Session templates: what a session is made of - its participants and
//! channels - before there is one. The model builds a session from a
//! template and the participants it names; finding or creating those
//! participants in the store is the runtime's (`SessionManager::
//! create_from_template`, issue 88), so nothing here touches a store.

use crate::actor::ActorId;
use crate::agent::PersonalityId;
use crate::approval::{ApprovalRule, ApprovalStrategy, Approver};
use crate::channel::{Channel, ChannelId, InternalActor, Participant};
use crate::component::ComponentConfig;
use crate::confidentiality::Classification;
use crate::privacy::PrivacyConfig;
use crate::project::Project;
use crate::removal::Removal;
use crate::scope::Scope::{GlobalScope, SessionScope, UserScope};
use crate::scope::{Scope, ScopeField, Scoped};
use crate::security::{AclEntry, Secured, SecurityAttributes};
use crate::session::{Session, SessionId};
use crate::store::{EntityKind, NoEntity, Stored};
use crate::user::UserId;
use crate::value::language::Language;
use crate::value::name::{Name, ScopedName};
use crate::value::text::{Briefing, Description, Summary};
use crate::value::time_zone::TimeZone;
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// A template that cannot be made into a session. The messages are meant
/// for users (see `caelumora_runtime::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// A channel names a participant the template does not have.
    #[error("channel participant '{0}' is not a participant of the template")]
    ParticipantNotFound(String),
    /// A user participant other than "user", which only the session's owner
    /// fills.
    #[error(
        "the template's user '{0}' is filled by nobody: only \"user\", the session's owner, is"
    )]
    UnboundUser(String),
    /// A scope that cannot be made yet.
    #[error("the scope '{0}' is not supported yet")]
    UnsupportedScope(&'static str),
    /// The agent the template names exists in the scope it asks for, but
    /// with another personality (issue 178): neither reused nor replaced.
    #[error(
        "the agent '{0}' already exists where the template asks for it, with another personality"
    )]
    AgentNameTaken(Name),
    /// The workspace the template names exists in the scope it asks for,
    /// but in another storage (issue 251): neither reused nor replaced.
    #[error(
        "the workspace '{0}' already exists where the template asks for it, in another storage"
    )]
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
    GlobalScope,
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
    /// unless named, the session's (issue 425).
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
    /// the template.
    pub participants: Vec<String>,
    /// The actors that belong to the channel itself (issue 420): its
    /// approver, if it names one - none that it does not name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_actors: Vec<InternalActorTemplate>,
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
    /// the entries of `access` besides those every channel has and the
    /// internal actors it names. Built empty (issue 420): the participants
    /// it names - by their names in the session template, `known`, not the
    /// user's or agent's own: the template's "user" is whoever the session
    /// was created for - join once it is made (`SessionTemplate::channels_of`).
    /// Held in the template's language, if it names one (issue 56).
    ///
    /// # Errors
    /// `TemplateError::ParticipantNotFound` if it names a participant that
    /// is not `known`.
    pub fn instantiate(
        &self,
        session_id: SessionId,
        owner: UserId,
        known: &[&str],
    ) -> TemplateResult<Channel> {
        if let Some(stray) = self.participants.iter().find(|name| !known.contains(&name.as_str())) {
            return Err(TemplateError::ParticipantNotFound(stray.clone()));
        }
        let channel = Channel::build(
            session_id,
            ActorId::User(owner),
            self.name.clone(),
            self.components.clone(),
            self.classification.clone(),
            self.language.clone(),
        );
        let mut security_attributes = channel.security_attributes.clone();
        security_attributes.acl.extend(self.access.iter().copied());
        let internal_actors =
            self.internal_actors.iter().map(InternalActorTemplate::instantiate).collect();
        Ok(Channel {
            approval_rules: self.approval_rules.clone(),
            summary: self.summary.clone(),
            briefing: self.briefing.clone(),
            uses_workspace: self.uses_workspace,
            security_attributes,
            internal_actors,
            ..channel
        })
    }
}

/// An internal actor of a channel as a template or a request names it
/// (issue 420): what it is made from, without the id it is given as the
/// channel is built.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InternalActorTemplate {
    /// An approver, deciding by `strategy` - `Any` unless named.
    Approver {
        #[serde(default)]
        strategy: ApprovalStrategy,
    },
}

impl InternalActorTemplate {
    /// The internal actor made from it, with an id of its own.
    pub fn instantiate(&self) -> InternalActor {
        match self {
            InternalActorTemplate::Approver { strategy } => {
                InternalActor::Approver(Approver::new(*strategy))
            }
        }
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
    /// The language sessions made from it are held in (issue 425), whoever
    /// creates them, unless the request names another; none in particular
    /// unless named - a frontend may then offer the one its creator prefers
    /// (issue 331).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// The time zone sessions made from it are held in (issue 424), unless
    /// the request names another; UTC unless named - a frontend may then
    /// offer its user's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<TimeZone>,
    /// Every participant of the session - users and agents - each with the
    /// name its channels refer to it by. A channel's approver is no
    /// participant: the channel names it (`ChannelTemplate::internal_actors`).
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
        Ok(ScopedName {
            name: self.name.clone(),
            scope: self.scope.instantiate(session_id, owner)?,
        })
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

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    fn validate(&self) -> Result<(), String> {
        match self.name.scope {
            Scope::SessionScope { .. } | Scope::ChannelScope { .. } => Err(format!(
                "the session template '{}' cannot lie in a session's or a channel's scope",
                self.name
            )),
            // Nothing of a project's scope is used yet (`Scope::admits`, issue 467).
            Scope::ProjectScope { .. } => Err(format!(
                "the session template '{}' cannot lie in a project's scope yet",
                self.name
            )),
            Scope::UserScope { .. } | Scope::GroupScope { .. } | Scope::GlobalScope => Ok(()),
        }
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
    /// Builds the session `id` of `owner`, named `name` and held in
    /// `language` and `time_zone` where the request names them, else in the
    /// template's (issues 424, 425), its own project, which its results go
    /// to (issue 467), and its channels -
    /// documents of their own, which the session lists by id (issue 70),
    /// built empty (issue 420). `participants` are the template's users as
    /// found for it (`ParticipantTemplate::request`), each with its name in
    /// the template: the session's. Its agents are not among them: they are
    /// found or made once the session is, and taken into it. Users and
    /// agents alike then join the channels that name them (`channels_of`).
    /// Nothing is stored: that is the caller's, so that it can announce the
    /// session where it is created.
    ///
    /// # Errors
    /// `TemplateError::ParticipantNotFound` if a channel names a participant
    /// the template does not have.
    pub fn build(
        &self,
        id: SessionId,
        owner: UserId,
        name: Name,
        language: Option<Language>,
        time_zone: Option<TimeZone>,
        participants: Vec<(String, Participant)>,
    ) -> TemplateResult<(Session, Project, Vec<Channel>)> {
        let known: Vec<&str> = self.participants.iter().map(ParticipantTemplate::name).collect();
        let channels = self
            .channels
            .iter()
            .map(|channel| channel.instantiate(id, owner, &known))
            .collect::<TemplateResult<Vec<_>>>()?;
        let participants: Vec<Participant> =
            participants.into_iter().map(|(_, participant)| participant).collect();
        let security_attributes = Session::attributes_of(id, owner, &self.access);
        let own = Project::own_of(id, owner);
        let now = Utc::now();
        let session = Session {
            id,
            name,
            summary: self.summary.clone(),
            briefing: self.briefing.clone(),
            language: language.or_else(|| self.language.clone()),
            time_zone: time_zone.or(self.time_zone),
            participants,
            channels: channels.iter().map(|c: &Channel| c.id).collect(),
            commands: self.commands.clone(),
            privacy: self.privacy,
            approval_rules: self.approval_rules.clone(),
            workspace: None,
            project: own.id,
            created_at: now,
            updated_at: now,
            security_attributes,
            deleted: None,
        };
        Ok((session, own, channels))
    }

    /// Of `channels` - those of a session `build` made, as it lists them, in
    /// the template's order - the ones whose template names the participant
    /// `name`.
    pub fn channels_of(&self, name: &str, channels: &[ChannelId]) -> Vec<ChannelId> {
        self.channels
            .iter()
            .zip(channels)
            .filter(|(template, _)| {
                template.participants.iter().any(|participant| participant == name)
            })
            .map(|(_, channel)| *channel)
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantTemplate {
    /// A user; "user" is the session's owner.
    User { name: String },
    /// An agent of `personality_id`, found by its name in `scope` or made.
    Agent { name: Name, scope: ScopeTemplate, personality_id: PersonalityId },
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
}

impl ParticipantTemplate {
    /// The name channels use to refer to this participant.
    pub fn name(&self) -> &str {
        match self {
            ParticipantTemplate::User { name } => name,
            ParticipantTemplate::Agent { name, .. } => name.as_str(),
        }
    }

    /// What this participant is in the session `session_id` of `owner`.
    ///
    /// # Errors
    /// `TemplateError::UnboundUser` for a user other than "user";
    /// `UnsupportedScope` for an agent of a scope that cannot be made.
    pub fn request(
        &self,
        session_id: SessionId,
        owner: UserId,
    ) -> TemplateResult<ParticipantRequest> {
        match self {
            ParticipantTemplate::User { name } if name == OWNER => {
                Ok(ParticipantRequest::User(owner))
            }
            ParticipantTemplate::User { name } => Err(TemplateError::UnboundUser(name.clone())),
            ParticipantTemplate::Agent { name, scope, personality_id } => {
                Ok(ParticipantRequest::Agent {
                    name: ScopedName {
                        name: name.clone(),
                        scope: scope.instantiate(session_id, owner)?,
                    },
                    personality_id: *personality_id,
                })
            }
        }
    }
}

/// Templates for the tests of this crate and of others, which write beside them only what they
/// differ in.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;
    use crate::channel::uses_workspace_by_default;

    /// A template `name` of the global scope for its "user" alone, without channels: a test names
    /// those it needs (`make_channel_template`).
    #[expect(
        clippy::expect_used,
        reason = "test code: a name that is none fails the test that gives it"
    )]
    pub fn make_template(name: &str) -> SessionTemplate {
        SessionTemplate {
            id: SessionTemplateId::new(),
            name: ScopedName::new(name, Scope::GlobalScope)
                .expect("a test's name of a template is one"),
            description: None,
            summary: None,
            briefing: None,
            language: None,
            time_zone: None,
            participants: vec![ParticipantTemplate::User { name: OWNER.to_string() }],
            channels: Vec::new(),
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            access: Vec::new(),
            security_attributes: Default::default(),
            deleted: None,
        }
    }

    /// A channel `name` of a template for its "user", with what a template written by hand leaves
    /// out: the default components and classification, the session's workspace, no approver.
    #[expect(
        clippy::expect_used,
        reason = "test code: a name that is none fails the test that gives it"
    )]
    pub fn make_channel_template(name: &str) -> ChannelTemplate {
        ChannelTemplate {
            name: Name::new(name).expect("a test's name of a channel is one"),
            summary: None,
            briefing: None,
            language: None,
            components: Default::default(),
            classification: Default::default(),
            participants: vec![OWNER.to_string()],
            internal_actors: Vec::new(),
            approval_rules: Vec::new(),
            uses_workspace: uses_workspace_by_default(),
            access: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{make_channel_template, make_template};
    use super::*;
    use crate::command::default_commands;
    use crate::confidentiality::Confidentiality;
    use uuid::Uuid;

    fn template() -> SessionTemplate {
        SessionTemplate {
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Agent {
                    name: Name::new("helper").unwrap(),
                    scope: ScopeTemplate::UserScope,
                    personality_id: PersonalityId::new(),
                },
            ],
            channels: vec![
                ChannelTemplate {
                    participants: vec!["user".to_string(), "helper".to_string()],
                    internal_actors: vec![InternalActorTemplate::Approver {
                        strategy: ApprovalStrategy::All,
                    }],
                    ..make_channel_template("main")
                },
                make_channel_template("side"),
            ],
            commands: default_commands(),
            ..make_template("pairing")
        }
    }

    /// The users as the runtime would resolve the requests; the agents join
    /// later.
    fn resolved(
        template: &SessionTemplate,
        session: SessionId,
        owner: UserId,
    ) -> Vec<(String, Participant)> {
        template
            .participants
            .iter()
            .filter_map(|p| {
                let participant = match p.request(session, owner).unwrap() {
                    ParticipantRequest::User(id) => Participant::User(id),
                    ParticipantRequest::Agent { .. } => return None,
                };
                Some((p.name().to_string(), participant))
            })
            .collect()
    }

    #[test]
    fn a_session_and_its_channels_are_described_as_their_templates_say() {
        let mut template = template();
        template.summary = Some(Summary::new("Pair programming").unwrap());
        template.briefing = Some(Briefing::new("Review before you merge.").unwrap());
        template.channels[0].summary = Some(Summary::new("Where the pairing happens").unwrap());
        let (id, owner) = (SessionId::new(), UserId::new());
        let (session, _, channels) = template
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&template, id, owner))
            .unwrap();
        assert_eq!(
            (
                session.summary.as_ref().map(Summary::as_str),
                session.briefing.as_ref().map(Briefing::as_str)
            ),
            (Some("Pair programming"), Some("Review before you merge.")),
            "copied"
        );
        assert_eq!(
            (channels[0].summary.as_ref().map(Summary::as_str), channels[0].briefing.as_ref()),
            (Some("Where the pairing happens"), None)
        );
        assert_eq!(channels[1].summary, None, "a channel that says nothing");

        let mut written = serde_json::to_value(&template).unwrap();
        written["channels"][1]["summary"] = serde_json::json!("two\nlines");
        let refused = serde_json::from_value::<SessionTemplate>(written).unwrap_err();
        assert!(
            refused.to_string().contains("the summary must be one line"),
            "a template written by hand is checked when it is read: {refused}"
        );
    }

    #[test]
    fn a_channel_is_held_in_the_language_its_template_names() {
        let mut template = template();
        template.channels[0].language = Some(Language::new("de").unwrap());
        let (id, owner) = (SessionId::new(), UserId::new());
        let (_, _, channels) = template
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&template, id, owner))
            .unwrap();
        assert_eq!(
            channels[0].language.as_ref().map(Language::as_str),
            Some("de"),
            "the template's"
        );
        assert_eq!(
            channels[1].language, None,
            "a template that names none: none of its own, the session's (issue 425)"
        );

        let named: ChannelTemplate = serde_json::from_value(
            serde_json::json!({"name": "main", "language": "de", "participants": []}),
        )
        .unwrap();
        assert_eq!(named.language, template.channels[0].language, "a seed names it by its tag");
        assert!(
            serde_json::from_value::<ChannelTemplate>(
                serde_json::json!({"name": "main", "language": "x", "participants": []})
            )
            .is_err(),
            "checked by its form"
        );
    }

    #[test]
    fn a_session_is_held_in_the_language_asked_for_else_in_its_templates() {
        let mut template = template();
        let (id, owner) = (SessionId::new(), UserId::new());
        let language = |tag: &str| Some(Language::new(tag).unwrap());
        let (session, _, _) = template
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&template, id, owner))
            .unwrap();
        assert_eq!(session.language, None, "neither names one: none in particular");
        template.language = language("de");
        let (session, _, _) = template
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&template, id, owner))
            .unwrap();
        assert_eq!(session.language, language("de"), "the template's");
        let (session, _, _) = template
            .build(
                id,
                owner,
                Name::new("s").unwrap(),
                language("fr"),
                None,
                resolved(&template, id, owner),
            )
            .unwrap();
        assert_eq!(session.language, language("fr"), "the one asked for before the template's");

        let named: SessionTemplate = serde_json::from_value(serde_json::json!({"id": Uuid::new_v4(), "name": "s", "language": "de", "participants": [], "channels": []})).unwrap();
        assert_eq!(named.language, language("de"), "a seed names it by its tag");
    }

    #[test]
    fn a_session_is_held_in_the_time_zone_asked_for_else_in_its_templates() {
        let mut template = template();
        let (id, owner) = (SessionId::new(), UserId::new());
        let zone = |name: &str| Some(TimeZone::new(name).unwrap());
        let (session, _, _) = template
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&template, id, owner))
            .unwrap();
        assert_eq!(session.time_zone, None, "neither names one: UTC");
        template.time_zone = zone("Europe/Berlin");
        let (session, _, _) = template
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&template, id, owner))
            .unwrap();
        assert_eq!(session.time_zone, zone("Europe/Berlin"), "the template's");
        let (session, _, _) = template
            .build(
                id,
                owner,
                Name::new("s").unwrap(),
                None,
                zone("Asia/Tokyo"),
                resolved(&template, id, owner),
            )
            .unwrap();
        assert_eq!(
            session.time_zone,
            zone("Asia/Tokyo"),
            "the one asked for before the template's"
        );

        let named: SessionTemplate = serde_json::from_value(serde_json::json!({"id": Uuid::new_v4(), "name": "s", "time_zone": "Europe/Berlin", "participants": [], "channels": []})).unwrap();
        assert_eq!(named.time_zone, zone("Europe/Berlin"), "a seed names it by its IANA name");
    }

    #[test]
    fn the_requests_name_the_owner_and_the_agents_scope() {
        let template = template();
        let (session, owner) = (SessionId::new(), UserId::new());
        let requests: Vec<_> =
            template.participants.iter().map(|p| p.request(session, owner).unwrap()).collect();

        assert!(
            matches!(requests[0], ParticipantRequest::User(id) if id == owner),
            "\"user\" is the owner"
        );
        assert!(
            matches!(&requests[1], ParticipantRequest::Agent { name: ScopedName { name, scope: Scope::UserScope { user_id } }, .. } if *name == "helper" && *user_id == owner)
        );

        let other = ParticipantTemplate::User { name: "guest".to_string() };
        assert!(
            matches!(other.request(session, owner), Err(TemplateError::UnboundUser(_))),
            "only the owner fills a user"
        );
        let group = ParticipantTemplate::Agent {
            name: Name::new("x").unwrap(),
            scope: ScopeTemplate::GroupScope,
            personality_id: PersonalityId::new(),
        };
        assert!(
            matches!(group.request(session, owner), Err(TemplateError::UnsupportedScope("group"))),
            "an error, not a panic"
        );
    }

    #[test]
    fn channels_refer_to_participants_by_their_template_name() {
        let template = template();
        let (id, owner) = (SessionId::new(), UserId::new());

        let (session, own, channels) = template
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&template, id, owner))
            .unwrap();
        assert_eq!(
            session.channels,
            channels.iter().map(|c| c.id).collect::<Vec<_>>(),
            "the session lists its channels by id"
        );
        assert_eq!(
            (session.project, own.owner()),
            (own.id, Some(owner)),
            "its results go to its own project (issue 467)"
        );
        assert_eq!(
            own.name.scope,
            Scope::SessionScope { session_id: id },
            "which lies in its scope"
        );
        assert_eq!(session.participants.len(), 1, "every user of the template");

        let approver = |c: &Channel| match c.internal_actors.as_slice() {
            [InternalActor::Approver(approver)] => Some(approver.strategy),
            _ => None,
        };
        assert!(
            channels.iter().all(|c| c.participants.is_empty()),
            "built empty: whom they name joins once they are made (issue 420)"
        );
        assert_eq!(
            template.channels_of("helper", &session.channels),
            [channels[0].id],
            "where it joins"
        );
        assert_eq!(
            template.channels_of("user", &session.channels),
            [channels[0].id, channels[1].id]
        );
        assert_eq!(
            approver(&channels[0]),
            Some(ApprovalStrategy::All),
            "an approver of its own, deciding as its template says"
        );
        assert!(
            channels[1].internal_actors.is_empty(),
            "a channel that names none has none: no fallback (issue 420)"
        );
        assert!(
            matches!(session.participants[0], Participant::User(u) if u == owner),
            "the template's \"user\" is the owner"
        );
        assert_eq!(
            session.commands.component_nodes.len(),
            default_commands().component_nodes.len(),
            "the session keeps its template's commands"
        );
        assert_eq!(channels[0].classification, Classification::CHANNEL_DEFAULT);
        assert_eq!(
            session.privacy,
            PrivacyConfig::Private,
            "private unless the template says otherwise"
        );
        let shared = SessionTemplate { privacy: PrivacyConfig::SharedAudience, ..template.clone() };
        let (session, _, _) = shared
            .build(id, owner, Name::new("s").unwrap(), None, None, resolved(&shared, id, owner))
            .unwrap();
        assert_eq!(session.privacy, PrivacyConfig::SharedAudience, "the template's policy");

        // A channel naming someone the template does not have.
        let mut stray = template.clone();
        stray.channels[1].participants.push("ghost".to_string());
        assert!(matches!(
            stray.build(
                id,
                owner,
                Name::new("x").unwrap(),
                None,
                None,
                resolved(&stray, id, owner)
            ),
            Err(TemplateError::ParticipantNotFound(_))
        ));

        let unnamed: ChannelTemplate =
            serde_json::from_value(serde_json::json!({"name": "main", "participants": []}))
                .unwrap();
        assert_eq!(
            unnamed.classification,
            Classification::general(Confidentiality::Internal),
            "the default"
        );
        assert!(unnamed.internal_actors.is_empty(), "none unless named");
        let approver: InternalActorTemplate =
            serde_json::from_value(serde_json::json!({"Approver": {}})).unwrap();
        assert!(
            matches!(approver, InternalActorTemplate::Approver { strategy: ApprovalStrategy::Any }),
            "the default strategy"
        );
    }
}
