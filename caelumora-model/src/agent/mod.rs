//! An agent: what it is made of, and what it keeps as it runs.
//!
//! - An `Agent` is a stored entity of its owner, acting in their name (issue
//!   341), made of a `Personality` (`personality`): its components, its kind
//!   and how it hears during a turn, read through the agent so that a change
//!   of the personality reaches it (issue 176).
//! - An `AgentChannelState` (`channel_state`) is the agent in one channel:
//!   its transcript, its notes, the turn under way, and the numbers its
//!   transcript gives attachments (`AttachmentIndex`, issue 282).
//! - An `AgentState` (`state`) is the agent itself across its channels: its
//!   memory (issue 62).

mod attachments;
mod channel_state;
mod personality;
mod state;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

use crate::actor::ActorId;
use crate::removal::Removal;
use crate::scope::{Scope, ScopeField, Scoped};
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::session::{ChannelId, SessionId};
use crate::store::{EntityKind, NoEntity, Revision, Stored, Updatable};
use crate::value::name::{Name, ScopedName};
use crate::value::ValueError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use attachments::AttachmentIndex;
pub use channel_state::{AgentChannelState, AgentChannelStateChange, RetryRefused};
pub use personality::{HearsDuringTurn, HearsFrom, OthersWords, Personality, PersonalityKind};
pub use state::{AgentState, AgentStateChange, STATE_CHANGED};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentUpdateEvent {
    /// Renamed, as it was asked: the agent's name is made from it as the
    /// change is applied (issue 248).
    NameChanged(String),
}

define_id!(PersonalityId);

define_id!(AgentId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    /// What it is found by (issue 177): its name, as every channel it takes
    /// part in knows it, and where it lies. A template finds an existing
    /// agent there, so the scope decides whether the agent is reused - one
    /// per user, with its memory - or made anew for every session.
    #[serde(flatten)]
    pub name: ScopedName,
    pub personality_id: PersonalityId,
    pub security_attributes: SecurityAttributes,
    /// In whose name the agent acts - whose rights it exercises (issue
    /// 341). Its owner when it is made, and its owner alone for now: whom
    /// else an agent may act for is issue 340.
    pub principal_id: ActorId,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
    /// Its revision (issue 492), counted up by every write of it.
    #[serde(default)]
    pub revision: Revision,
}

impl Agent {
    /// The context the agent acts in when nobody asked it, in the channel
    /// `channel` of session `session` (issue 87): begun by the agent itself,
    /// answered for by its owner and acting in the name of its principal
    /// (issues 255, 341).
    pub fn security_context(&self, session: SessionId, channel: ChannelId) -> SecurityContext {
        let agent = ActorId::Agent(self.id);
        SecurityContext::new(agent, session, channel).next(
            agent,
            self.security_attributes.owner_id,
            self.principal_id,
        )
    }

    /// A new agent of the personality `personality_id`, owned by `owner` -
    /// whoever creates it - and acting in their name (issue 341). Nothing of
    /// the personality is copied into it: what the agent is - its
    /// components, its introduction - is read through `personality_id`, so
    /// that a change of the personality reaches it (issue 176).
    pub fn new(name: ScopedName, personality_id: PersonalityId, owner: ActorId) -> Self {
        let agent_id = AgentId(Uuid::new_v4());
        // Read by its owner, whose rights it acts with (issue 255): it reads
        // its own definition as its owner.
        let security_attributes = SecurityAttributes::owned_by(owner);
        Agent {
            id: agent_id,
            personality_id,
            name,
            security_attributes,
            principal_id: owner,
            deleted: None,
            revision: Default::default(),
        }
    }
}

impl Stored for Agent {
    const KIND: EntityKind = EntityKind::new("agent");
    type Id = AgentId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> AgentId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    fn revision(&self) -> Revision {
        self.revision
    }

    fn set_revision(&mut self, revision: Revision) {
        self.revision = revision;
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }

    /// An agent acts in its owner's name (issue 341): whoever may change it
    /// cannot lend it another's rights by naming them as its principal. Nor
    /// is the system its owner: only the system itself acts in the system's
    /// name (issue 477). It lies in no project's scope, which nothing is used
    /// from yet (`Scope::admits`, issue 467): it would take part nowhere.
    fn validate(&self) -> Result<(), String> {
        if self.principal_id != self.security_attributes.owner_id {
            return Err("an agent acts in its owner's name".to_string());
        }
        if self.security_attributes.owner_id == ActorId::System {
            return Err("the system owns no agent".to_string());
        }
        if matches!(self.name.scope, Scope::ProjectScope { .. }) {
            return Err("an agent lies in no project's scope yet".to_string());
        }
        Ok(())
    }
}

/// What a change of the agent says is made into its values here (issue
/// 248).
impl Updatable for Agent {
    type Update = AgentUpdateEvent;

    fn apply(&mut self, event: &AgentUpdateEvent) -> Result<(), ValueError> {
        match event {
            AgentUpdateEvent::NameChanged(name) => self.name.name = Name::new(name)?,
        }
        Ok(())
    }
}

impl Secured for Agent {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for Agent {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectId;
    use crate::user::UserId;

    /// Issue 341: an agent acting when nobody asked it is the caller and
    /// the initiator - it caused what it does -, answered for by its owner
    /// and acting in its owner's name.
    #[test]
    fn an_agent_acting_on_its_own_initiates_it_for_its_owner() {
        let owner = ActorId::User(UserId::new());
        let agent = Agent::new(
            ScopedName::new("helper", Default::default()).unwrap(),
            PersonalityId::new(),
            owner,
        );
        let context = agent.security_context(Default::default(), Default::default());
        let itself = ActorId::Agent(agent.id);
        assert_eq!(
            (context.caller_id, context.initiator_id),
            (itself, itself),
            "it acts, and caused it"
        );
        assert_eq!(
            (context.owner_id, context.principal_id),
            (owner, owner),
            "answered for by its owner, in their name"
        );
    }

    /// Issue 341: an agent is made acting in its owner's name, and cannot
    /// be stored acting in anybody else's - whom else it may act for is
    /// issue 340.
    #[test]
    fn an_agent_acts_in_its_owners_name() {
        let owner = ActorId::User(UserId::new());
        let mut agent = Agent::new(
            ScopedName::new("helper", Default::default()).unwrap(),
            PersonalityId::new(),
            owner,
        );
        assert_eq!(agent.principal_id, owner, "its owner's when made");
        assert!(agent.validate().is_ok());
        agent.principal_id = ActorId::User(UserId::new());
        assert!(agent.validate().is_err(), "not lent another's rights");
    }

    /// Issue 477: no agent acts with the system's rights.
    #[test]
    fn an_agent_is_not_the_systems() {
        let name = ScopedName::new("helper", Default::default()).unwrap();
        let systems = Agent::new(name, PersonalityId::new(), ActorId::System);
        assert!(systems.validate().is_err(), "made in the system's name, owned by it");
    }

    /// Issue 467: what lies in a project's scope is used nowhere yet, so an
    /// agent, which would take part nowhere, does not lie there.
    #[test]
    fn an_agent_lies_in_no_project_s_scope() {
        let project = Scope::ProjectScope { project_id: ProjectId::new() };
        let agent = Agent::new(
            ScopedName::new("helper", project).unwrap(),
            PersonalityId::new(),
            ActorId::User(UserId::new()),
        );
        assert!(agent.validate().is_err());
    }
}
