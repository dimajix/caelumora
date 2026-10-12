use crate::agent::AgentId;
use crate::channel::{ChannelId, ChannelUpdateEvent};
use crate::confidentiality::Classification;
use crate::intern::intern;
use crate::removal::Stored;
use crate::security::SecurityAttributes;
use crate::session::SessionId;
use crate::store::EntityKind;
use crate::user::UserId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tracing::warn;

/// What happened, announced on the event bus - and, once there are several
/// instances, between them (issue 122): it is read back as it is written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalEvent {
    /// About an entity named by its own id (issue 323), of whatever kind:
    /// a session, an agent, a user, a user's profile, a workspace, a skill,
    /// an artifact, a group, or a kind of a plugin. Published with the
    /// entity's attributes, so that whoever may read it receives it:
    ///
    /// - a session's and an agent's: an agent of user or global scope
    ///   belongs to no session and takes part in several (issue 121); a
    ///   restored one takes part in no channel until it is added again;
    /// - a profile's: only its user, and the runtime (issue 13); it comes
    ///   as it now stands;
    /// - a workspace's (issue 253), a skill's (issue 22) and a group's
    ///   (issue 286): its owner and whom its access list grants reading,
    ///   a group's members; the participants of a session or channel are
    ///   announced as they join and leave, not as a group;
    /// - an artifact's (issue 271): its owner, and for a version published
    ///   the readers of that version.
    ///
    /// `id` is the entity's id as text, which `about` reads back.
    Entity { kind: EntityKind, id: String, change: EntityChange },
    ChannelCreated(SessionId, ChannelId),
    ChannelUpdated(SessionId, ChannelId, ChannelUpdateEvent),
    ChannelDeleted(SessionId, ChannelId),
    /// A deleted channel was restored into its session (issue 164).
    ChannelRestored(SessionId, ChannelId),
    ChannelStarted(SessionId, ChannelId),
    ChannelStopped(SessionId, ChannelId),
    AgentChannelJoined(SessionId, ChannelId, AgentId),
    AgentChannelLeft(SessionId, ChannelId, AgentId),
    /// The state of an agent in a channel changed - its notes or private
    /// notes (issue 65). Published with the channel's attributes once it is
    /// stored, before the agent announces the end of its turn; without
    /// content, since part of it is private.
    AgentChannelStateChanged(SessionId, ChannelId, AgentId),
    UserChannelJoined(SessionId, ChannelId, UserId),
    UserChannelLeft(SessionId, ChannelId, UserId),
}

/// What happened to an entity (issue 323). The four of its life the same
/// for every kind; what changed in an update, and what is particular to a
/// kind, in the form the kind gives it - read back by `update` and `data`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum EntityChange {
    Created,
    /// Changed, as the kind's update event says - `SkillUpdateEvent`, …
    Updated(Value),
    /// Deleted - marked, or removed for what is not marked (issue 164).
    Deleted,
    /// A deleted one was restored (issue 164): it is there again, as it was.
    Restored,
    /// Something particular to the kind, under a name of its own - a
    /// session started (`session::STARTED`), an agent's state changed
    /// (`agent::STATE_CHANGED`) -, with what the kind says of it, `null`
    /// for nothing.
    Custom { name: &'static str, data: Value },
}

/// Read as it is written, the name of a custom change kept as `intern`
/// keeps it - which serde does not do for a field of a `&'static str`.
impl<'de> Deserialize<'de> for EntityChange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        enum Written {
            Created,
            Updated(Value),
            Deleted,
            Restored,
            Custom { name: String, data: Value },
        }
        Ok(match Written::deserialize(deserializer)? {
            Written::Created => Self::Created,
            Written::Updated(update) => Self::Updated(update),
            Written::Deleted => Self::Deleted,
            Written::Restored => Self::Restored,
            Written::Custom { name, data } => Self::Custom { name: intern(&name), data },
        })
    }
}

impl GlobalEvent {
    /// The event of `change` to the entity of kind `T` with `id`.
    pub fn entity<T: Stored>(id: &T::Id, change: EntityChange) -> Self {
        Self::Entity { kind: T::KIND, id: id.to_string(), change }
    }

    /// The entity of kind `T` with `id` was created.
    pub fn created<T: Stored>(id: &T::Id) -> Self {
        Self::entity::<T>(id, EntityChange::Created)
    }

    /// The entity of kind `T` with `id` was changed, as `update` says.
    pub fn updated<T: Stored>(id: &T::Id, update: &impl Serialize) -> Self {
        Self::entity::<T>(id, EntityChange::Updated(value_of(T::KIND, update)))
    }

    /// The entity of kind `T` with `id` was deleted.
    pub fn deleted<T: Stored>(id: &T::Id) -> Self {
        Self::entity::<T>(id, EntityChange::Deleted)
    }

    /// The entity of kind `T` with `id` was restored.
    pub fn restored<T: Stored>(id: &T::Id) -> Self {
        Self::entity::<T>(id, EntityChange::Restored)
    }

    /// What `name` says happened to the entity of kind `T` with `id`, with
    /// `data`.
    pub fn custom<T: Stored>(id: &T::Id, name: &'static str, data: &impl Serialize) -> Self {
        Self::entity::<T>(id, EntityChange::Custom { name, data: value_of(T::KIND, data) })
    }

    /// The id of the entity of kind `T` this event is about, and what
    /// happened to it; `None` for an event about anything else - or about
    /// an id that is none of `T`'s.
    pub fn about<T: Stored>(&self) -> Option<(T::Id, &EntityChange)> {
        match self {
            Self::Entity { kind, id, change } if *kind == T::KIND => id.parse().ok().map(|id| (id, change)),
            _ => None,
        }
    }
}

impl EntityChange {
    /// What an update says, in the form of the kind's update event `U`;
    /// `None` for another change, or one that is no `U`.
    pub fn update<U: DeserializeOwned>(&self) -> Option<U> {
        match self {
            Self::Updated(update) => U::deserialize(update).ok(),
            _ => None,
        }
    }

    /// What the custom change `name` says, as `D`; `None` for another
    /// change, or data that is no `D`.
    pub fn data<D: DeserializeOwned>(&self, name: &str) -> Option<D> {
        match self {
            Self::Custom { name: this, data } if *this == name => D::deserialize(data).ok(),
            _ => None,
        }
    }
}

/// `value` as an event carries it; `null` if it cannot be written, which a
/// kind's own events never are - noted in the log, since the event goes all
/// the same.
fn value_of(kind: EntityKind, value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or_else(|e| {
        warn!(%kind, "An event goes without what changed: {}", e);
        Value::Null
    })
}

pub trait EventPublisher: Send + Sync + std::fmt::Debug {
    /// Publishes `event` about an entity with these `attributes` - the
    /// session's, channel's, agent's, user's or workspace's the event is
    /// about (issue 39).
    /// They decide who may receive it; they are not part of the event.
    fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes);

    /// Publishes `event` about a classified entity - a channel and what
    /// happens in it, a workspace, a skill, an artifact: received only by
    /// whoever may read it and is cleared for one of `classifications`, as
    /// the views reach it (issue 312). One classification, the entity's;
    /// two as it is reclassified, the one it had and the one it has, so
    /// that whoever no longer reaches it learns so.
    fn publish_classified(&self, event: GlobalEvent, attributes: SecurityAttributes, classifications: &[Classification]);
}

impl EventPublisher for () {
    fn publish(&self, _event: GlobalEvent, _attributes: SecurityAttributes) {}

    fn publish_classified(&self, _event: GlobalEvent, _attributes: SecurityAttributes, _classifications: &[Classification]) {}
}

#[cfg(test)]
mod tests {
    use super::{EntityChange, GlobalEvent};
    use crate::session::{self, Session, SessionId, SessionUpdateEvent};
    use serde_json::json;

    /// Issue 122: an event is read back as it was written - by another
    /// instance, which knows the kind and the custom change by name alone.
    #[test]
    fn an_event_is_read_back_as_it_was_written() {
        let id = SessionId::new();
        let renamed = GlobalEvent::updated::<Session>(&id, &SessionUpdateEvent::NameChanged("Release".to_string()));
        let written = serde_json::to_value(&renamed).unwrap();
        assert_eq!(written, json!({"Entity": {"kind": "session", "id": id.to_string(), "change": {"Updated": {"NameChanged": "Release"}}}}));

        let read: GlobalEvent = serde_json::from_value(written).unwrap();
        let (read_id, change) = read.about::<Session>().expect("about a session");
        assert_eq!(read_id, id);
        assert!(matches!(change.update::<SessionUpdateEvent>(), Some(SessionUpdateEvent::NameChanged(name)) if name == "Release"));

        let started = GlobalEvent::custom::<Session>(&id, session::STARTED, &());
        let read: GlobalEvent = serde_json::from_str(&serde_json::to_string(&started).unwrap()).unwrap();
        assert!(matches!(read.about::<Session>(), Some((_, EntityChange::Custom { name: session::STARTED, .. }))), "{read:?}");
    }
}
