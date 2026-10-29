//! A skill (issue 22): a named, self-contained piece of procedural
//! knowledge - "how we write a release note" - that exists apart from any
//! personality and is loaded into a turn when it is needed. Instructions
//! only: a skill tells the model how to do something with the tools it has,
//! and brings none of its own. One classification holds for the whole of it.

use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::define_id;
use caelumora_model::digest::{Digest, Encoder};
use caelumora_model::removal::Removal;
use caelumora_model::scope::{Scope, ScopeField, Scoped};
use caelumora_model::security::{CustomVerb, Grant, Requirement, Secured, SecurityAttributes};
use caelumora_model::snapshot::ScopeSnapshot;
use caelumora_model::store::{EntityKind, NoEntity, Revision, Stored};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::{Description, Text, TextRule};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

define_id!(SkillId);

/// Loading a skill into a turn (issue 22): what the agent then follows,
/// asked in the turn's context - the agent for its owner, whoever asked as
/// initiator. Apart from reading, so that a policy can let a skill be read
/// but not used, or the other way round; granted as reading by default.
/// Whether its classification may flow into the channel is asked besides
/// (`may_publish`).
pub const USE: CustomVerb = CustomVerb { name: "Use", requires: Requirement::Grant(Grant::Read) };

/// A change of a skill, as it was asked: what it says is made into the
/// skill's values as it is applied (issue 248).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillUpdateEvent {
    /// Renamed.
    NameChanged(String),
    /// What it is for, in a line, as it now stands.
    DescriptionChanged(String),
    /// What it tells the model, as it now stands.
    BodyChanged(String),
    /// Reclassified as a whole: lowering releases it, raising retracts
    /// nothing already loaded.
    ClassificationChanged(Classification),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: SkillId,
    /// What it is found by and where it lies: the global scope, a user's -
    /// its owner's - or a session's, made there and gone with it; its name
    /// unique there (issue 250). No channel's: a skill serves a session's
    /// channels alike.
    #[serde(flatten)]
    pub name: ScopedName,
    /// What it is for, in a line: what the catalogue shows the model, which
    /// decides by it whether to load the skill.
    pub description: Description,
    /// What it tells the model, given to it only when it is loaded.
    pub body: SkillBody,
    /// How the whole of it is classified: where it may be loaded, and what
    /// a turn that loaded it has seen.
    pub classification: Classification,
    /// Its owner - a user, or the system for a skill of the seed - and an
    /// access list of those it is shared with, who read and use it. Left out in a
    /// seed, whose skills are the system's (issue 491).
    #[serde(default = "SecurityAttributes::systems")]
    pub security_attributes: SecurityAttributes,
    /// When it was made and last changed; left out in a seed, written by
    /// hand, when it is read.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
    /// Its revision (issue 492), counted up by every write of it.
    #[serde(default)]
    pub revision: Revision,
}

impl Skill {
    /// A new skill of `owner`, of `name` where it lies, for what
    /// `description` says, telling `body`, classified `classification`,
    /// shared with nobody.
    pub fn new(
        name: ScopedName,
        description: Description,
        body: SkillBody,
        classification: Classification,
        owner: ActorId,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: SkillId::new(),
            name,
            description,
            body,
            classification,
            security_attributes: SecurityAttributes::owned_by(owner),
            created_at: now,
            updated_at: now,
            deleted: None,
            revision: Default::default(),
        }
    }
}

impl Stored for Skill {
    const KIND: EntityKind = EntityKind::new("skill");
    type Id = SkillId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> SkillId {
        self.id
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
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

    /// A skill belongs to a user or to the system. A user's lies in the
    /// global scope, a session's or their own; the system's - the seed's -
    /// in the global scope alone, where everybody finds it.
    fn validate(&self) -> Result<(), String> {
        match (self.security_attributes.owner_id, &self.name.scope) {
            (ActorId::User(_), Scope::GlobalScope | Scope::SessionScope { .. }) => Ok(()),
            (ActorId::User(owner), Scope::UserScope { user_id }) if *user_id == owner => Ok(()),
            (ActorId::User(_), Scope::UserScope { .. }) => {
                Err("a skill lies in no other user's scope than its owner's".to_string())
            }
            (
                ActorId::User(_),
                Scope::ChannelScope { .. } | Scope::GroupScope { .. } | Scope::ProjectScope { .. },
            ) => Err("a skill lies in the global scope, a session's or its owner's".to_string()),
            (ActorId::System, Scope::GlobalScope) => Ok(()),
            (ActorId::System, _) => {
                Err("a skill of the system lies in the global scope".to_string())
            }
            _ => Err("a skill belongs to a user or to the system".to_string()),
        }
    }

    fn classification(&self) -> Option<&Classification> {
        Some(&self.classification)
    }
}

impl Secured for Skill {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for Skill {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

/// What a skill tells the model to do (issue 22): instructions in Markdown,
/// loaded into a turn when the skill is used - twice a briefing, enough for
/// a procedure, not for a manual.
pub enum SkillBodyRule {}

impl TextRule for SkillBodyRule {
    const FIELD: &'static str = "body";
    const MAX_CHARS: usize = 32_000;
    const ONE_LINE: bool = false;
}

pub type SkillBody = Text<SkillBodyRule>;

/// The digest of a skill's body as it was loaded (issue 22).
pub fn digest(body: &str) -> Digest {
    let mut e = Encoder::new("caelumora.skill.v1");
    e.str(body);
    e.finish()
}

/// A skill as the viewer sees it (issue 22), without its body, which is
/// read on its own.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillSnapshot {
    pub id: SkillId,
    pub name: String,
    /// Where it lies: the global scope, its owner's or a session's.
    pub scope: ScopeSnapshot,
    pub description: Description,
    pub classification: Classification,
    /// Whose it is.
    pub owner: ActorId,
    /// Whether the viewer may change it - rename, describe, rewrite,
    /// reclassify - and delete it (`UpdateSkill`).
    pub can_change: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::session::{ChannelId, SessionId};
    use caelumora_model::user::UserId;

    fn skill(scope: Scope, owner: ActorId) -> Skill {
        Skill::new(
            ScopedName::new("release-notes", scope).unwrap(),
            Description::new("How we write a release note").unwrap(),
            SkillBody::new("1. Collect the merged changes.\n2. Group them by area.").unwrap(),
            Classification::CHANNEL_DEFAULT,
            owner,
        )
    }

    #[test]
    fn a_users_skill_lies_in_the_global_scope_a_sessions_or_their_own() {
        let alice = UserId::new();
        let session_id = SessionId::new();
        for scope in [
            Scope::GlobalScope,
            Scope::SessionScope { session_id },
            Scope::UserScope { user_id: alice },
        ] {
            assert!(skill(scope.clone(), ActorId::User(alice)).validate().is_ok(), "{scope:?}");
        }
        for scope in [
            Scope::UserScope { user_id: UserId::new() },
            Scope::ChannelScope { session_id, channel_id: ChannelId::new() },
        ] {
            assert!(skill(scope.clone(), ActorId::User(alice)).validate().is_err(), "{scope:?}");
        }
    }

    #[test]
    fn a_skill_of_the_system_lies_in_the_global_scope_and_nobody_elses_is_stored() {
        assert!(skill(Scope::GlobalScope, ActorId::System).validate().is_ok());
        assert!(skill(Scope::SessionScope { session_id: SessionId::new() }, ActorId::System)
            .validate()
            .is_err());
        assert!(skill(Scope::GlobalScope, ActorId::None).validate().is_err(), "nobody's");
        assert!(
            skill(Scope::GlobalScope, ActorId::Agent(AgentId::new())).validate().is_err(),
            "an agent's: its owner's"
        );
    }
}
