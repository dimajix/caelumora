use crate::actor::ActorId;
use crate::agent::Agent;
use crate::approval::Approver;
use crate::event::{EventPublisher, GlobalEvent};
use crate::message::MessageHistory;
use crate::security::{Secured, SecurityAttributes};
use crate::session::SessionId;
use crate::store::EntityStore;
use crate::user::User;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Something that may not be done in a channel.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// The `SecurityManager` refused the action. Deliberately silent about
    /// which rule refused it and about what the caller would have to be
    /// instead - like a refused tool call (issue 15).
    #[error("{action} channel {channel} is not permitted")]
    Forbidden { action: &'static str, channel: ChannelId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct ChannelController {
    id: ChannelId,
    session_id: SessionId,
    channel: ArcSwap<Channel>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Option<Arc<dyn EntityStore>>,
}

impl ChannelController {
    pub fn new(channel: Channel, event_publisher: Arc<dyn EventPublisher>, entity_store: Option<Arc<dyn EntityStore>>) -> Self {
        let id = channel.id;
        let session_id = channel.session_id;
        Self {
            id,
            session_id,
            channel: ArcSwap::from_pointee(channel),
            event_publisher,
            entity_store,
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Channel>> {
        self.channel.load()
    }

    pub fn id(&self) -> ChannelId {
        self.id
    }

    pub fn name(&self) -> String {
        self.channel.load().name.clone()
    }

    pub async fn update_name(&self, new_name: String) {
        let event = ChannelUpdateEvent::NameChanged(new_name);
        self.process_channel_update(&event).await;
        self.event_publisher.publish(GlobalEvent::ChannelUpdated(self.session_id, self.id, event));
    }

    async fn process_channel_update(&self, event: &ChannelUpdateEvent) {
        match event {
            ChannelUpdateEvent::NameChanged(name) => {
                let mut channel = (**self.channel.load()).clone();
                channel.name = name.clone();
                self.channel.store(Arc::new(channel));
            }
        }
    }

    pub async fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::ChannelUpdated(_, cid, update) if *cid == self.id => {
                self.process_channel_update(update).await;
            }
            GlobalEvent::AgentChannelJoined(_, cid, aid) if *cid == self.id => {
                if let Some(store) = &self.entity_store {
                    let security = crate::security::SecurityContext::default();
                    if let Ok(agent) = store.get_agent(&security, *aid).await {
                        let mut channel = (**self.channel.load()).clone();
                        if !channel.participants.iter().any(|p| match p {
                            Participant::Agent(a) => a.id == *aid,
                            _ => false,
                        }) {
                            channel.participants.push(Participant::Agent(agent));
                            self.channel.store(Arc::new(channel));
                        }
                    }
                }
            }
            GlobalEvent::AgentChannelLeft(_, cid, aid) if *cid == self.id => {
                let mut channel = (**self.channel.load()).clone();
                let initial_len = channel.participants.len();
                channel.participants.retain(|p| match p {
                    Participant::Agent(a) => a.id != *aid,
                    _ => true,
                });
                if channel.participants.len() != initial_len {
                    self.channel.store(Arc::new(channel));
                }
            }
            GlobalEvent::UserChannelJoined(_, cid, uid) if *cid == self.id => {
                if let Some(store) = &self.entity_store {
                    let security = crate::security::SecurityContext::default();
                    if let Ok(user) = store.get_user(&security, *uid).await {
                        let mut channel = (**self.channel.load()).clone();
                        if !channel.participants.iter().any(|p| match p {
                            Participant::User(u) => u.id == *uid,
                            _ => false,
                        }) {
                            channel.participants.push(Participant::User(user));
                            self.channel.store(Arc::new(channel));
                        }
                    }
                }
            }
            GlobalEvent::UserChannelLeft(_, cid, uid) if *cid == self.id => {
                let mut channel = (**self.channel.load()).clone();
                let initial_len = channel.participants.len();
                channel.participants.retain(|p| match p {
                    Participant::User(u) => u.id != *uid,
                    _ => true,
                });
                if channel.participants.len() != initial_len {
                    self.channel.store(Arc::new(channel));
                }
            }
            _ => {}
        }
    }
}

define_id!(ChannelId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub session_id: SessionId,
    pub name: String,
    pub moderator: String,
    pub participants: Vec<Participant>,
    pub security_attributes: SecurityAttributes,
}

impl Secured for Channel {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Channel {
    pub fn new(session_id: SessionId, name: String, moderator: String, participants: Vec<Participant>, security_attributes: SecurityAttributes) -> Self {
        let id = ChannelId(Uuid::new_v4());
        Self {
            id,
            session_id,
            name,
            moderator,
            participants,
            security_attributes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_id: ChannelId,
    pub history: MessageHistory,
}

impl ChannelState {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            history: MessageHistory::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User(User),
    Agent(Agent),
    Approver(Approver),
}

impl Participant {
    pub fn actor_id(&self) -> ActorId {
        match self {
            Participant::User(user) => ActorId::User(user.id),
            Participant::Agent(agent) => ActorId::Agent(agent.id),
            Participant::Approver(approver) => ActorId::Approver(approver.id.0),
        }
    }
}

/// The attributes of a session or channel owned by `owner` with these
/// participants: its users and agents contribute - they read it and act in
/// it - while only the owner may change or delete it (issue 11). Approvers
/// act on behalf of users and need no access of their own.
pub fn participant_attributes(owner: ActorId, participants: &[Participant]) -> SecurityAttributes {
    let mut contributors: Vec<ActorId> = Vec::new();
    for participant in participants {
        let id = participant.actor_id();
        if matches!(participant, Participant::User(_) | Participant::Agent(_)) && id != owner && !contributors.contains(&id) {
            contributors.push(id);
        }
    }
    SecurityAttributes { owner_id: owner, contributors }
}
