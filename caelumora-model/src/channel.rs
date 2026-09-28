use crate::actor::ActorId;
use crate::agent::Agent;
use crate::approval::{ApprovalStrategy, Approver, ApproverId};
use crate::confidentiality::{ChannelClassification, Confidentiality};
use crate::event::{EventPublisher, GlobalEvent};
use crate::message::MessageHistory;
use crate::security::{Secured, SecurityAttributes};
use crate::session::SessionId;
use crate::security::SecurityContext;
use crate::store::{EntityStore, StoreResult};
use crate::user::User;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// The moderator of a channel whose template or request names none.
pub const DEFAULT_MODERATOR: &str = "round_robin";

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

/// The one writer of a channel's document (issue 70): changes are stored
/// first - where the permission is checked, on the channel's own attributes
/// - and only then taken over live and announced.
#[derive(Debug)]
pub struct ChannelController {
    id: ChannelId,
    session_id: SessionId,
    channel: ArcSwap<Channel>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Option<Arc<dyn EntityStore>>,
    /// Held while the channel is changed and stored, so that two changes
    /// cannot overwrite each other.
    write_lock: tokio::sync::Mutex<()>,
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
            write_lock: tokio::sync::Mutex::new(()),
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

    /// Renames the channel on behalf of `ctx` (`UpdateChannel`).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the channel is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: String) -> StoreResult<()> {
        let event = ChannelUpdateEvent::NameChanged(new_name);
        self.update(ctx, &event).await
    }

    /// Applies `event`, stores the channel, takes it over and announces it.
    async fn update(&self, ctx: &SecurityContext, event: &ChannelUpdateEvent) -> StoreResult<()> {
        let _writing = self.write_lock.lock().await;
        let mut channel = (**self.channel.load()).clone();
        match event {
            ChannelUpdateEvent::NameChanged(name) => channel.name = name.clone(),
        }
        if let Some(store) = &self.entity_store {
            store.update_channel(ctx, &channel).await?;
        }
        let attributes = channel.security_attributes.clone();
        self.channel.store(Arc::new(channel));
        self.event_publisher.publish(GlobalEvent::ChannelUpdated(self.session_id, self.id, event.clone()), attributes);
        Ok(())
    }

    pub async fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::AgentChannelJoined(_, cid, aid) if *cid == self.id => {
                if let Some(store) = &self.entity_store {
                    // Keeping the channel in step with an event: the join
                    // itself was permitted where it happened.
                    let security = crate::security::SecurityContext::system();
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
                    // Keeping the channel in step with an event: the join
                    // itself was permitted where it happened.
                    let security = crate::security::SecurityContext::system();
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
    /// How far the channel's contents may travel (issue 61); never
    /// `Private`. Set by its owner when creating it.
    #[serde(default = "default_confidentiality")]
    pub confidentiality: Confidentiality,
}

fn default_confidentiality() -> Confidentiality {
    Confidentiality::CHANNEL_DEFAULT
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
            confidentiality: Confidentiality::CHANNEL_DEFAULT,
        }
    }

    /// The channel as the confidentiality rules see it.
    pub fn classification(&self) -> ChannelClassification {
        ChannelClassification { session_id: self.session_id, channel_id: self.id, level: self.confidentiality }
    }

    /// A new channel of `participants`, owned by `owner`, as every channel is
    /// built - from a template or on request (issue 09): with an approver
    /// added if there is none, and with every user and agent in it as a
    /// contributor (see `participant_attributes`).
    pub fn build(session_id: SessionId, owner: ActorId, name: String, moderator: String, confidentiality: Confidentiality, mut participants: Vec<Participant>) -> Self {
        if !participants.iter().any(|p| matches!(p, Participant::Approver(_))) {
            participants.push(Participant::Approver(Approver {
                id: ApproverId(Uuid::new_v4()),
                strategy: ApprovalStrategy::Any,
            }));
        }
        let security_attributes = participant_attributes(owner, &participants);
        Self { confidentiality, ..Self::new(session_id, name, moderator, participants, security_attributes) }
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
