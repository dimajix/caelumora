//! The actors of a channel: who they are, what passes between them, and how far each is in its
//! work. An actor is known by its id (`ActorId`), and what of it is public to its channel is its
//! description (`ActorDescription`); what it sends and is sent is `event`'s (`ActorEvent`), how an
//! event travels `envelope`'s (`Envelope`, `EventContext`), how far it is `state`'s (`ActorState`).

mod envelope;
mod event;
mod state;

pub use envelope::{Envelope, EventContext, Pick};
pub use event::{ActorEvent, Rejection, RequestReason, RetryTarget};
pub use state::{ActorState, ProcessingStateHolder};

use crate::agent::AgentId;
use crate::approval::ApproverId;
use crate::mention::handle_of;
use crate::query::FieldValue;
use crate::user::UserId;
use crate::value::name::Name;
use crate::value::text::Introduction;
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActorId {
    #[default]
    None,
    User(UserId),
    Agent(AgentId),
    Approver(ApproverId),
    /// The kernel itself, acting on no one's behalf: setting up initial data, keeping its own
    /// records in step with events. Only ever set deliberately, through
    /// `SecurityContext::system()` - never the default, so that a context nobody filled in has no
    /// rights (issue 11).
    System,
}
/// An actor as a query compares it, after serde's tags: `{"User": "…"}`,
/// `"System"`. A participant, whose variants have the same form, is asked
/// by the actor it is.
impl From<ActorId> for FieldValue {
    fn from(actor: ActorId) -> Self {
        match actor {
            ActorId::None => FieldValue::from("None"),
            ActorId::User(id) => FieldValue::tagged("User", id),
            ActorId::Agent(id) => FieldValue::tagged("Agent", id),
            ActorId::Approver(id) => FieldValue::tagged("Approver", id),
            ActorId::System => FieldValue::from("System"),
        }
    }
}

impl ActorId {
    /// The name every approver is shown with.
    pub const APPROVER_NAME: &'static str = "Approver";

    /// The name every approver is shown with, as a name: what an approver's
    /// actor is called in its channel.
    pub fn approver_name() -> Name {
        Name::fixed(Self::APPROVER_NAME)
    }

    /// The name of an actor that is no entity and so has no name of its own: an approver, the
    /// kernel itself, nobody. `None` for users and agents, whose names are their entities'
    /// (`ControllerManager::actor_name`).
    pub fn fixed_name(&self) -> Option<&'static str> {
        match self {
            ActorId::Approver(_) => Some(Self::APPROVER_NAME),
            ActorId::System => Some("system"),
            ActorId::None => Some("unknown"),
            ActorId::User(_) | ActorId::Agent(_) => None,
        }
    }

    /// The id of the actor - the nil UUID for the system and for nobody,
    /// who have none of their own.
    pub fn id(&self) -> Uuid {
        match &self {
            ActorId::None => Uuid::nil(),
            ActorId::User(id) => id.0,
            ActorId::Agent(id) => id.0,
            ActorId::Approver(id) => id.0,
            ActorId::System => Uuid::nil(),
        }
    }

    pub fn as_agent_id(&self) -> Option<AgentId> {
        match self {
            ActorId::Agent(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_user_id(&self) -> Option<UserId> {
        match self {
            ActorId::User(id) => Some(*id),
            _ => None,
        }
    }
}

/// What of an actor is public to everyone in its channel (issues 176, 403):
/// who it is, what it is called and how it is introduced. Whatever is added
/// to it must be public in the same way, since it is given to whom only
/// reads the channel's roster - a moderator's policy, another agent's
/// prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct ActorDescription {
    pub id: ActorId,
    pub name: Name,
    /// An agent as its personality introduces it, a user as they introduce
    /// themselves; `None` for one without an introduction.
    pub introduction: Option<Introduction>,
}

impl ActorDescription {
    /// The handle it is `@`-mentioned by (`mention::handle_of`), or `None`
    /// if it cannot be mentioned: one that is neither a user nor an agent,
    /// and a name that yields none.
    pub fn handle(&self) -> Option<String> {
        matches!(self.id, ActorId::User(_) | ActorId::Agent(_))
            .then(|| handle_of(self.name.as_str()))
            .filter(|handle| !handle.is_empty())
    }

    /// Whether it is a person: a user, not an agent, an approver or the
    /// system.
    pub fn is_person(&self) -> bool {
        matches!(self.id, ActorId::User(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::meets_exactly_its_equals;

    /// Issue 403: what is public of an actor names its handle only where it
    /// can be mentioned, and says whether it is a person.
    #[test]
    fn a_description_names_a_handle_where_its_actor_can_be_mentioned() {
        let described = |id, name: &str| ActorDescription {
            id,
            name: Name::new(name).unwrap(),
            introduction: None,
        };
        let zoe = described(ActorId::User(UserId::new()), "Zoë Smith");
        assert_eq!((zoe.handle().as_deref(), zoe.is_person()), (Some("zoe-smith"), true));
        let helper = described(ActorId::Agent(AgentId::new()), "Helper");
        assert_eq!((helper.handle().as_deref(), helper.is_person()), (Some("helper"), false));
        let approver = described(ActorId::Approver(ApproverId::new()), "Approver");
        assert_eq!(
            (approver.handle(), approver.is_person()),
            (None, false),
            "an approver is not mentioned"
        );
    }

    #[test]
    fn the_approvers_name_is_a_name() {
        assert_eq!(
            Name::new(ActorId::APPROVER_NAME).map(|name| name.as_str().to_string()),
            Ok(ActorId::approver_name().as_str().to_string())
        );
        assert_eq!(ActorId::approver_name(), "approver");
    }

    /// An actor as a query compares it (issue 293): the variant counts, not
    /// only the UUID.
    #[test]
    fn an_actor_meets_exactly_the_actors_equal_to_it() {
        let id = Uuid::new_v4();
        meets_exactly_its_equals(&[
            ActorId::None,
            ActorId::System,
            ActorId::User(UserId(id)),
            ActorId::User(UserId::new()),
            ActorId::Agent(AgentId(id)),
            ActorId::Approver(ApproverId(id)),
        ]);
    }
}
