use crate::confidentiality::Classification;
use crate::intern::intern;
use crate::security::SecurityAttributes;
use crate::store::{Address, Enclosing, EntityKind, Stored};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt::Debug;
use std::slice;
use tracing::warn;

/// What happened, announced on the event bus - and, once there are several instances, between them
/// (issue 122): it is read back as it is written. Every event is about an entity (issue 323), of
/// whatever kind: a session, a channel, an agent, a user, a user's profile, a workspace, a project,
/// a skill, an artifact, a group, or a kind of a plugin. Published with the entity's attributes, so
/// that whoever may read it receives it:
///
/// - a session's and an agent's: an agent of user or global scope belongs to no session and takes
///   part in several (issue 121); a restored one takes part in no channel until it is added again;
/// - a channel's, and what happens to its participants, by the channel's (issue 304);
/// - a profile's: only its user, and the runtime (issue 13); it comes as it now stands;
/// - a workspace's (issue 253), a skill's (issue 22) and a group's (issue 286): its owner and whom
///   its access list grants reading, a group's participants; the participants of a session or
///   channel are announced as they join and leave, not as a group;
/// - a project's (issue 467): its owner and whom its list grants reading - its members, the
///   participants of the sessions in it -; a change of its members to who left as well;
/// - an artifact's (issue 271): its owner and whom its list grants reading - the members of its
///   project among them (issue 467) -, and for a version published the readers of that version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalEvent {
    /// The kind of the entity.
    pub kind: EntityKind,
    /// The entity's id as text; `about` reads it back.
    pub id: String,
    /// The id of the entity it lies in - a channel's session - for a kind
    /// that lies in one (issue 335); `about` reads it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<String>,
    /// What happened to it.
    pub change: EntityChange,
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
    /// session started (`session::STARTED`), someone joined a channel
    /// (`channel::JOINED`), an agent's state changed
    /// (`agent::STATE_CHANGED`) -, with what the kind says of it, `null`
    /// for nothing.
    Custom {
        name: &'static str,
        data: Value,
    },
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
    /// The event of `change` to the entity of kind `T` at `address`: its
    /// id, or for a kind that lies in an entity that entity's id and its
    /// own (issue 335).
    pub fn entity<T: Stored>(address: &Address<T>, change: EntityChange) -> Self {
        let (within, id) = T::Within::write(address);
        Self { kind: T::KIND, id, within, change }
    }

    /// The entity of kind `T` at `address` was created.
    pub fn created<T: Stored>(address: &Address<T>) -> Self {
        Self::entity::<T>(address, EntityChange::Created)
    }

    /// The entity of kind `T` at `address` was changed, as `update` says.
    pub fn updated<T: Stored>(address: &Address<T>, update: &impl Serialize) -> Self {
        Self::entity::<T>(address, EntityChange::Updated(value_of(T::KIND, update)))
    }

    /// The entity of kind `T` at `address` was deleted.
    pub fn deleted<T: Stored>(address: &Address<T>) -> Self {
        Self::entity::<T>(address, EntityChange::Deleted)
    }

    /// The entity of kind `T` at `address` was restored.
    pub fn restored<T: Stored>(address: &Address<T>) -> Self {
        Self::entity::<T>(address, EntityChange::Restored)
    }

    /// What `name` says happened to the entity of kind `T` at `address`,
    /// with `data`.
    pub fn custom<T: Stored>(
        address: &Address<T>,
        name: &'static str,
        data: &impl Serialize,
    ) -> Self {
        Self::entity::<T>(address, EntityChange::Custom { name, data: value_of(T::KIND, data) })
    }

    /// The address of the entity of kind `T` this event is about - its id,
    /// or the id of the entity it lies in and its own -, and what happened
    /// to it; `None` for an event about another kind - or about an address
    /// that is none of `T`'s.
    pub fn about<T: Stored>(&self) -> Option<(Address<T>, &EntityChange)> {
        if self.kind != T::KIND {
            return None;
        }
        T::Within::read(self.within.as_deref(), &self.id).map(|address| (address, &self.change))
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

pub trait EventPublisher: Send + Sync + Debug {
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
    fn publish_classified(
        &self,
        event: GlobalEvent,
        attributes: SecurityAttributes,
        classifications: &[Classification],
    );
}

impl EventPublisher for () {
    fn publish(&self, _event: GlobalEvent, _attributes: SecurityAttributes) {}

    fn publish_classified(
        &self,
        _event: GlobalEvent,
        _attributes: SecurityAttributes,
        _classifications: &[Classification],
    ) {
    }
}

/// The one way an event about an entity is published as the entity stands
/// (issue 351): to whoever may read it, and of a classified kind is cleared
/// for it - what an event is announced with is the entity's to say, not its
/// caller's. What is announced otherwise says why: a reclassification to
/// who reached the entity as it was, too (issue 312), a leave to whoever
/// left besides (issue 23). By blanket implementation on every
/// `EventPublisher`, like `Enforcement` on every `AccessPolicy`.
pub trait Announcing {
    /// Publishes `event` about `entity` with its attributes and, if its kind
    /// is classified, its classification.
    fn announce<T: Stored>(&self, event: GlobalEvent, entity: &T);
}

impl<P: EventPublisher + ?Sized> Announcing for P {
    fn announce<T: Stored>(&self, event: GlobalEvent, entity: &T) {
        let classifications = entity.classification().map(slice::from_ref).unwrap_or_default();
        self.publish_classified(event, entity.security_attributes().clone(), classifications);
    }
}

#[cfg(test)]
mod tests {
    use super::{Announcing, EntityChange, EventPublisher, GlobalEvent};
    use crate::actor::ActorId;
    use crate::channel::{self, Channel, ChannelId};
    use crate::confidentiality::Classification;
    use crate::scope::Scope;
    use crate::security::{Secured, SecurityAttributes};
    use crate::session::{self, Session, SessionId, SessionUpdateEvent};
    use crate::user::{UserId, UserProfile};
    use crate::value::name::{Name, ScopedName};
    use crate::workspace::Workspace;
    use serde_json::json;
    use std::sync::Mutex;

    /// What was published, with whom it reaches.
    #[derive(Debug, Default)]
    struct Published(Mutex<Vec<(SecurityAttributes, Vec<Classification>)>>);

    impl EventPublisher for Published {
        fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes) {
            self.publish_classified(event, attributes, &[]);
        }

        fn publish_classified(
            &self,
            _event: GlobalEvent,
            attributes: SecurityAttributes,
            classifications: &[Classification],
        ) {
            self.0.lock().unwrap().push((attributes, classifications.to_vec()));
        }
    }

    /// Issue 351: an entity is announced with its own attributes, and with
    /// its classification if its kind has one.
    #[test]
    fn an_entity_is_announced_as_it_stands() {
        let owner = UserId::new();
        let workspace = Workspace::new(
            ScopedName { name: Name::new("notes").unwrap(), scope: Scope::default() },
            "local",
            Classification::PUBLIC,
            owner,
        );
        let profile = UserProfile::new(owner, None);
        let published = Published::default();
        published.announce(GlobalEvent::created::<Workspace>(&workspace.id), &workspace);
        published.announce(GlobalEvent::created::<UserProfile>(&owner), &profile);
        assert_eq!(
            *published.0.lock().unwrap(),
            [
                (workspace.security_attributes.clone(), vec![Classification::PUBLIC]),
                (profile.security_attributes().clone(), vec![]),
            ],
            "a classified kind to who is cleared for it, another to whoever may read it"
        );
    }

    /// Issue 122: an event is read back as it was written - by another
    /// instance, which knows the kind and the custom change by name alone.
    #[test]
    fn an_event_is_read_back_as_it_was_written() {
        let id = SessionId::new();
        let renamed = GlobalEvent::updated::<Session>(
            &id,
            &SessionUpdateEvent::NameChanged("Release".to_string()),
        );
        let written = serde_json::to_value(&renamed).unwrap();
        assert_eq!(
            written,
            json!({"kind": "session", "id": id.to_string(), "change": {"Updated": {"NameChanged": "Release"}}})
        );

        let read: GlobalEvent = serde_json::from_value(written).unwrap();
        let (read_id, change) = read.about::<Session>().expect("about a session");
        assert_eq!(read_id, id);
        assert!(
            matches!(change.update::<SessionUpdateEvent>(), Some(SessionUpdateEvent::NameChanged(name)) if name == "Release")
        );

        let started = GlobalEvent::custom::<Session>(&id, session::STARTED, &());
        let read: GlobalEvent =
            serde_json::from_str(&serde_json::to_string(&started).unwrap()).unwrap();
        assert!(
            matches!(
                read.about::<Session>(),
                Some((_, EntityChange::Custom { name: session::STARTED, .. }))
            ),
            "{read:?}"
        );
    }

    /// Issue 335: an event about a channel names the session it lies in
    /// beside the channel, and is read back as both.
    #[test]
    fn an_event_names_what_its_entity_lies_in() {
        let session = SessionId::new();
        let channel = ChannelId::new();
        let user = UserId::new();
        let joined = GlobalEvent::custom::<Channel>(
            &(session, channel),
            channel::JOINED,
            &ActorId::User(user),
        );
        let written = serde_json::to_value(&joined).unwrap();
        assert_eq!(
            written,
            json!({"kind": "channel", "id": channel.to_string(), "within": session.to_string(), "change": {"Custom": {"name": "joined", "data": {"User": user.to_string()}}}})
        );

        let read: GlobalEvent = serde_json::from_value(written).unwrap();
        let ((read_session, read_channel), change) =
            read.about::<Channel>().expect("about a channel");
        assert_eq!((read_session, read_channel), (session, channel));
        assert_eq!(change.data::<ActorId>(channel::JOINED), Some(ActorId::User(user)));

        let unplaced: GlobalEvent = serde_json::from_value(
            json!({"kind": "channel", "id": channel.to_string(), "change": "Created"}),
        )
        .unwrap();
        assert!(unplaced.about::<Channel>().is_none(), "a channel lies in a session");
        let placed: GlobalEvent = serde_json::from_value(json!({"kind": "session", "id": session.to_string(), "within": channel.to_string(), "change": "Created"})).unwrap();
        assert!(placed.about::<Session>().is_none(), "a session lies in nothing");
    }
}
