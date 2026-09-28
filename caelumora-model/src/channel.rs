use crate::actor::ActorId;
use crate::agent::Agent;
use crate::approval::{ApprovalStrategy, Approver, ApproverId};
use crate::confidentiality::{ChannelClassification, Confidentiality};
use crate::event::{EventPublisher, GlobalEvent};
use crate::message::{Message, MessageHistory, MessageId};
use chrono::{DateTime, Utc};
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
    /// Only participants of the session can be added to its channels
    /// (issue 46).
    #[error("{participant} is not a participant of the session of channel {channel}")]
    NotInSession { participant: ActorId, channel: ChannelId },
    #[error("{participant} already takes part in channel {channel}")]
    AlreadyParticipant { participant: ActorId, channel: ChannelId },
    #[error("{participant} takes no part in channel {channel}")]
    NotAParticipant { participant: ActorId, channel: ChannelId },
    /// Every channel keeps its approver.
    #[error("the approver of channel {channel} cannot be removed")]
    ApproverStays { channel: ChannelId },
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

    /// Adds `participant` to the channel on behalf of `ctx` (issue 46), seeing
    /// its history from `start` on (`None`: all of it). The channel's
    /// attributes are derived anew from its participants, so the newcomer
    /// may read it. Stored first (`UpdateChannel`), then taken over and
    /// announced (`AgentChannelJoined` / `UserChannelJoined`).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the channel is then unchanged.
    pub async fn add_participant(&self, ctx: &SecurityContext, participant: Participant, start: Option<HistoryStart>) -> StoreResult<()> {
        let _writing = self.write_lock.lock().await;
        let mut channel = (**self.channel.load()).clone();
        let id = participant.actor_id();
        channel.participants.push(participant);
        channel.history_limits.retain(|l| l.participant != id);
        if let Some(start) = start {
            channel.history_limits.push(HistoryLimit { participant: id, start });
        }
        channel.security_attributes = participant_attributes(channel.security_attributes.owner_id, &channel.participants);
        self.store(ctx, &channel).await?;
        let attributes = channel.security_attributes.clone();
        self.channel.store(Arc::new(channel));
        if let Some(event) = self.membership_event(id, true) {
            self.event_publisher.publish(event, attributes);
        }
        Ok(())
    }

    /// Removes `participant` from the channel on behalf of `ctx` (issue 46)
    /// and returns it, like [`Self::add_participant`]. The announcement
    /// (`AgentChannelLeft` / `UserChannelLeft`) carries the attributes the
    /// channel had, so that whoever was removed hears of it too.
    ///
    /// # Errors
    /// `StoreError::NotFound` if `participant` takes no part in the channel;
    /// whatever the store reports otherwise.
    pub async fn remove_participant(&self, ctx: &SecurityContext, participant: ActorId) -> StoreResult<Participant> {
        let _writing = self.write_lock.lock().await;
        let mut channel = (**self.channel.load()).clone();
        let index = channel.participants.iter().position(|p| p.actor_id() == participant)
            .ok_or_else(|| crate::store::StoreError::NotFound { entity: "participant", id: participant.to_string() })?;
        let removed = channel.participants.remove(index);
        channel.history_limits.retain(|l| l.participant != participant);
        let before = channel.security_attributes.clone();
        channel.security_attributes = participant_attributes(before.owner_id, &channel.participants);
        self.store(ctx, &channel).await?;
        self.channel.store(Arc::new(channel));
        if let Some(event) = self.membership_event(participant, false) {
            self.event_publisher.publish(event, before);
        }
        Ok(removed)
    }

    /// The event announcing that `participant` joined or left.
    fn membership_event(&self, participant: ActorId, joined: bool) -> Option<GlobalEvent> {
        match (participant, joined) {
            (ActorId::Agent(a), true) => Some(GlobalEvent::AgentChannelJoined(self.session_id, self.id, a)),
            (ActorId::Agent(a), false) => Some(GlobalEvent::AgentChannelLeft(self.session_id, self.id, a)),
            (ActorId::User(u), true) => Some(GlobalEvent::UserChannelJoined(self.session_id, self.id, u)),
            (ActorId::User(u), false) => Some(GlobalEvent::UserChannelLeft(self.session_id, self.id, u)),
            _ => None,
        }
    }

    /// Stores `channel` on behalf of `ctx`, if the controller has a store.
    async fn store(&self, ctx: &SecurityContext, channel: &Channel) -> StoreResult<()> {
        match &self.entity_store {
            Some(store) => store.update_channel(ctx, channel).await,
            None => Ok(()),
        }
    }

    /// Applies `event`, stores the channel, takes it over and announces it.
    async fn update(&self, ctx: &SecurityContext, event: &ChannelUpdateEvent) -> StoreResult<()> {
        let _writing = self.write_lock.lock().await;
        let mut channel = (**self.channel.load()).clone();
        match event {
            ChannelUpdateEvent::NameChanged(name) => channel.name = name.clone(),
        }
        self.store(ctx, &channel).await?;
        let attributes = channel.security_attributes.clone();
        self.channel.store(Arc::new(channel));
        self.event_publisher.publish(GlobalEvent::ChannelUpdated(self.session_id, self.id, event.clone()), attributes);
        Ok(())
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
    /// Where the view of the history begins for participants who joined
    /// later and may not see all of it (issue 46); everybody else sees all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_limits: Vec<HistoryLimit>,
}

/// How much of a channel's history a new participant may see, as asked for
/// when adding them (issue 46).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryAccess {
    #[default]
    All,
    None,
    LastDays { count: u32 },
    LastMessages { count: u32 },
}

impl HistoryAccess {
    /// Where the view of `history` begins, fixed now - so that "the last
    /// three days" does not move day by day. `None` if all of it is visible.
    pub fn resolve(&self, history: &[Message], now: DateTime<Utc>) -> Option<HistoryStart> {
        let last = || history.last().map(|m| HistoryStart::After(m.id));
        let from = |index: usize| match index {
            0 => None,
            i if i >= history.len() => last(),
            i => Some(HistoryStart::From(history[i].id)),
        };
        match *self {
            HistoryAccess::All => None,
            HistoryAccess::None => last(),
            HistoryAccess::LastMessages { count } => from(history.len().saturating_sub(count as usize)),
            HistoryAccess::LastDays { count } => {
                let since = now - chrono::Duration::days(i64::from(count));
                from(history.iter().position(|m| m.timestamp >= since).unwrap_or(history.len()))
            }
        }
    }
}

/// Where a participant's view of a channel's history begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStart {
    /// From this message on.
    From(MessageId),
    /// After this message: what came before the participant joined is not
    /// visible.
    After(MessageId),
}

impl HistoryStart {
    /// The part of `history` visible from here - nothing if the message it
    /// begins at is not there, rather than more than was granted.
    pub fn visible(&self, history: Vec<Message>) -> Vec<Message> {
        let (id, skip) = match *self {
            HistoryStart::From(id) => (id, 0),
            HistoryStart::After(id) => (id, 1),
        };
        match history.iter().position(|m| m.id == id) {
            Some(index) => history.into_iter().skip(index + skip).collect(),
            None => Vec::new(),
        }
    }
}

/// The view of the history granted to one participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryLimit {
    pub participant: ActorId,
    pub start: HistoryStart,
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
            history_limits: Vec::new(),
        }
    }

    /// The part of `history` - this channel's - that `participant` may see:
    /// all of it, unless they joined later with a limit (issue 46).
    pub fn visible_history(&self, participant: ActorId, history: Vec<Message>) -> Vec<Message> {
        match self.history_limits.iter().find(|l| l.participant == participant) {
            Some(limit) => limit.start.visible(history),
            None => history,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Five messages, one a day, the last one `now`.
    fn history(now: DateTime<Utc>) -> Vec<Message> {
        (0..5).rev()
            .map(|days| Message { id: MessageId::new(), timestamp: now - chrono::Duration::days(days), ..Message::default() })
            .collect()
    }

    fn visible(access: HistoryAccess, history: &[Message], now: DateTime<Utc>) -> Vec<MessageId> {
        let messages = history.to_vec();
        let messages = match access.resolve(history, now) {
            Some(start) => start.visible(messages),
            None => messages,
        };
        messages.into_iter().map(|m| m.id).collect()
    }

    fn ids(history: &[Message]) -> Vec<MessageId> {
        history.iter().map(|m| m.id).collect()
    }

    #[test]
    fn history_access_all_and_none_grant_everything_and_nothing() {
        let now = Utc::now();
        let history = history(now);
        assert_eq!(visible(HistoryAccess::All, &history, now), ids(&history), "All sees every message");
        assert!(visible(HistoryAccess::None, &history, now).is_empty(), "None sees no earlier message");
        assert_eq!(HistoryAccess::None.resolve(&[], now), None, "an empty channel has nothing to hide");
    }

    #[test]
    fn history_access_last_messages_and_days_cut_the_history() {
        let now = Utc::now();
        let history = history(now);
        assert_eq!(visible(HistoryAccess::LastMessages { count: 2 }, &history, now), ids(&history[3..]));
        assert_eq!(visible(HistoryAccess::LastMessages { count: 9 }, &history, now), ids(&history),
                   "more than there is grants all of it");
        assert!(visible(HistoryAccess::LastMessages { count: 0 }, &history, now).is_empty());
        // Messages 1, 2 and 3 days ago plus today's - within the last three days.
        assert_eq!(visible(HistoryAccess::LastDays { count: 3 }, &history, now), ids(&history[1..]));
        assert!(visible(HistoryAccess::LastDays { count: 3 }, &history, now + chrono::Duration::days(10)).is_empty(),
                "nothing within the last days sees nothing");
    }

    #[test]
    fn history_start_shows_later_messages_and_nothing_if_its_message_is_gone() {
        let now = Utc::now();
        let history = history(now);
        let start = HistoryStart::After(history[1].id);
        let mut later = history.clone();
        later.push(Message { id: MessageId::new(), ..Message::default() });
        assert_eq!(ids(&start.visible(later.clone())), ids(&later[2..]), "messages sent after joining stay visible");
        assert!(HistoryStart::From(MessageId::new()).visible(later).is_empty(),
                "an unknown start shows nothing rather than everything");
    }
}
