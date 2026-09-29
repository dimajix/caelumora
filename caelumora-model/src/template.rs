//! Session templates: what a session is made of - its participants and
//! channels - before there is one. The model builds a session from a
//! template and the participants it names; finding or creating those
//! participants in the store is the runtime's (`SessionManager::
//! create_from_template`, issue 88), so nothing here touches a store.

use crate::agent::PersonalityId;
use crate::approval::{ApprovalStrategy, Approver, ApproverId};
use crate::channel::{participant_attributes, Channel, Participant};
use crate::component::ComponentConfig;
use crate::confidentiality::Confidentiality;
use crate::meta::Scope::{GlobalScope, SessionScope, UserScope};
use crate::meta::{Metadata, Scope};
use crate::session::{Session, SessionId};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::actor::ActorId;
use crate::security::{Secured, SecurityAttributes};
use crate::user::UserId;

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
    /// A channel of the template has a level only an actor may have.
    #[error("channel '{channel}' cannot be {level}: that level is an actor's, not a channel's")]
    InvalidConfidentiality { channel: String, level: Confidentiality },
    /// A scope that cannot be made yet.
    #[error("the scope '{0}' is not supported yet")]
    UnsupportedScope(&'static str),
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
    /// The scope for the session `session_id` of `owner`.
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
    pub name: String,
    #[serde(default = "default_moderator")]
    pub moderator: String,
    /// How far the channel's contents may travel (issue 61); `session`
    /// unless named.
    #[serde(default = "default_confidentiality")]
    pub confidentiality: Confidentiality,
    /// The session template's participants that take part, by their name in
    /// the template. A channel that names no approver gets one anyway
    /// (`Channel::build`).
    pub participants: Vec<String>,
}

fn default_moderator() -> String {
    crate::channel::DEFAULT_MODERATOR.to_string()
}

fn default_confidentiality() -> Confidentiality {
    Confidentiality::CHANNEL_DEFAULT
}

impl ChannelTemplate {
    /// Builds the channel of session `session_id`, owned by `owner`.
    /// `session_participants` pairs each participant of the session with
    /// its name in the session template, which is what `self.participants`
    /// refers to - not the user's or agent's own name: the template's "user"
    /// is whoever the session was created for.
    pub fn instantiate(&self, session_id: SessionId, owner: UserId, session_participants: &[(String, Participant)]) -> TemplateResult<Channel> {
        if !self.confidentiality.is_channel_level() {
            return Err(TemplateError::InvalidConfidentiality { channel: self.name.clone(), level: self.confidentiality });
        }
        let participants = self.participants.iter()
            .map(|name| session_participants.iter()
                .find(|(template_name, _)| template_name == name)
                .map(|(_, participant)| participant.clone())
                .ok_or_else(|| TemplateError::ParticipantNotFound(name.clone())))
            .collect::<TemplateResult<Vec<_>>>()?;
        Ok(Channel::build(session_id, ActorId::User(owner), self.name.clone(), self.moderator.clone(), self.confidentiality, participants))
    }
}

define_id!(SessionTemplateId);

/// What sessions are made from. Written by hand (seed files): the fields
/// with a default may be left out (issue 108).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTemplate {
    pub id: SessionTemplateId,
    pub metadata: Metadata,
    pub scope: ScopeTemplate,
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
}

impl Secured for SessionTemplate {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl SessionTemplate {
    /// Builds the session `id` of `owner`, named `name`, and its channels -
    /// documents of their own, which the session lists by id (issue 70).
    /// `participants` are the template's participants as found or created
    /// for it (`ParticipantTemplate::request`), each with its name in the
    /// template. Nothing is stored: that is the caller's, so that it can
    /// announce the session where it is created.
    pub fn build(&self, id: SessionId, owner: UserId, name: String, participants: Vec<(String, Participant)>) -> TemplateResult<(Session, Vec<Channel>)> {
        let channels = self.channels.iter()
            .map(|channel| channel.instantiate(id, owner, &participants))
            .collect::<TemplateResult<Vec<_>>>()?;
        let participants: Vec<Participant> = participants.into_iter().map(|(_, participant)| participant).collect();
        let security_attributes = participant_attributes(ActorId::User(owner), &participants);
        let now = Utc::now();
        let session = Session {
            id,
            metadata: Metadata {
                name,
                scope: self.scope.instantiate(id, owner)?,
                description: self.metadata.description.clone(),
                ..Metadata::default()
            },
            participants,
            channels: channels.iter().map(|c: &Channel| c.id).collect(),
            commands: self.commands.clone(),
            created_at: now,
            updated_at: now,
            security_attributes,
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
        name: String,
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
    /// The agent named `name` in `scope`, or a new one of `personality_id`.
    Agent { name: String, scope: Scope, personality_id: PersonalityId },
    /// This approver, which needs nothing from the store.
    Approver(Approver),
}

impl ParticipantTemplate {
    /// The name channels use to refer to this participant.
    pub fn name(&self) -> &str {
        match self {
            ParticipantTemplate::User { name } | ParticipantTemplate::Agent { name, .. } | ParticipantTemplate::Approver { name, .. } => name,
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
                name: name.clone(),
                scope: scope.instantiate(session_id, owner)?,
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
            security_attributes: Default::default(),
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata::default(),
            scope: ScopeTemplate::SessionScope,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Agent { name: "helper".to_string(), scope: ScopeTemplate::UserScope, personality_id: PersonalityId::new() },
                ParticipantTemplate::Approver { name: "approval".to_string(), strategy: ApprovalStrategy::All },
            ],
            channels: vec![
                ChannelTemplate {
                    name: "main".to_string(),
                    moderator: "broadcast".to_string(),
                    confidentiality: Confidentiality::CHANNEL_DEFAULT,
                    participants: vec!["user".to_string(), "helper".to_string(), "approval".to_string()],
                },
                ChannelTemplate {
                    name: "side".to_string(),
                    moderator: "broadcast".to_string(),
                    confidentiality: Confidentiality::CHANNEL_DEFAULT,
                    participants: vec!["user".to_string()],
                },
            ],
            commands: crate::command::default_commands(),
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
    fn the_requests_name_the_owner_and_the_agents_scope() {
        let template = template();
        let (session, owner) = (SessionId::new(), UserId::new());
        let requests: Vec<_> = template.participants.iter().map(|p| p.request(session, owner).unwrap()).collect();

        assert!(matches!(requests[0], ParticipantRequest::User(id) if id == owner), "\"user\" is the owner");
        assert!(matches!(&requests[1], ParticipantRequest::Agent { name, scope: Scope::UserScope { user_id }, .. } if name == "helper" && *user_id == owner));
        assert!(matches!(&requests[2], ParticipantRequest::Approver(a) if a.strategy == ApprovalStrategy::All), "the template's strategy");

        let other = ParticipantTemplate::User { name: "guest".to_string() };
        assert!(matches!(other.request(session, owner), Err(TemplateError::UnboundUser(_))), "only the owner fills a user");
        let group = ParticipantTemplate::Agent { name: "x".to_string(), scope: ScopeTemplate::GroupScope, personality_id: PersonalityId::new() };
        assert!(matches!(group.request(session, owner), Err(TemplateError::UnsupportedScope("group"))), "an error, not a panic");
    }

    #[test]
    fn channels_refer_to_participants_by_their_template_name() {
        let template = template();
        let (id, owner) = (SessionId::new(), UserId::new());

        let (session, channels) = template.build(id, owner, "s".to_string(), resolved(&template, id, owner)).unwrap();
        assert_eq!(session.channels, channels.iter().map(|c| c.id).collect::<Vec<_>>(), "the session lists its channels by id");
        assert_eq!(session.participants.len(), 3, "every actor of the template");

        let approvers = |c: &Channel| c.participants.iter().filter(|p| matches!(p, Participant::Approver(_))).count();
        assert_eq!(channels[0].participants.len(), 3);
        assert_eq!(approvers(&channels[0]), 1, "the template's approver, not a second one");
        assert_eq!(approvers(&channels[1]), 1, "a channel that names none gets one");
        assert!(matches!(channels[1].participants[0], Participant::User(u) if u == owner), "the template's \"user\" is the owner");
        assert_eq!(session.commands.component_nodes.len(), 2, "the session keeps its template's commands");
        assert_eq!(channels[0].confidentiality, Confidentiality::Session);

        // A channel naming someone the template does not have.
        let mut stray = template.clone();
        stray.channels[1].participants.push("ghost".to_string());
        assert!(matches!(stray.build(id, owner, "x".to_string(), resolved(&stray, id, owner)), Err(TemplateError::ParticipantNotFound(_))));

        // A channel cannot be private: that level is an actor's.
        let mut private = template.clone();
        private.channels[0].confidentiality = Confidentiality::Private;
        let refused = private.build(id, owner, "p".to_string(), resolved(&private, id, owner)).expect_err("a private channel");
        assert!(matches!(refused, TemplateError::InvalidConfidentiality { .. }), "{refused}");
        let unnamed: ChannelTemplate = serde_json::from_value(serde_json::json!({"name": "main", "participants": []})).unwrap();
        assert_eq!(unnamed.confidentiality, Confidentiality::Session, "the default");
        let approver: ParticipantTemplate = serde_json::from_value(serde_json::json!({"Approver": {"name": "approval"}})).unwrap();
        assert!(matches!(approver, ParticipantTemplate::Approver { strategy: ApprovalStrategy::Any, .. }), "the default strategy");

    }
}
