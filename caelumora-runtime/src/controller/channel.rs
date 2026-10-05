use caelumora_model::name::Name;
use arc_swap::ArcSwap;
use caelumora_model::language::Language;
use caelumora_model::confidentiality::Classification;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{participant_attributes, Channel, ChannelId, ChannelUpdateEvent, FormerParticipant, HistoryLimit, HistoryStart, Participant};
use caelumora_model::removal::Removal;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, StoreError, StoreResult};
use std::sync::Arc;

/// The one writer of a channel's document (issue 70): changes are stored
/// first - where the permission is checked, on the channel's own attributes
/// - and only then taken over live and announced.
#[derive(Debug)]
pub struct ChannelController {
    id: ChannelId,
    session_id: SessionId,
    channel: ArcSwap<Channel>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
    /// Held while the channel is changed and stored, so that two changes
    /// cannot overwrite each other.
    write_lock: tokio::sync::Mutex<()>,
}

impl ChannelController {
    pub fn new(channel: Channel, event_publisher: Arc<dyn EventPublisher>, entity_store: Arc<dyn EntityStore>) -> Self {
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

    pub fn name(&self) -> Name {
        self.channel.load().name.clone()
    }

    /// Renames the channel on behalf of `ctx` (`UpdateChannel`).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the channel is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: Name) -> StoreResult<()> {
        let event = ChannelUpdateEvent::NameChanged(new_name);
        self.update(ctx, &event).await
    }

    /// Reclassifies the channel on behalf of `ctx` (`UpdateChannel`; whether
    /// it may be reclassified at all is its `ChannelLoop`'s to ask, issue
    /// 133).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the channel is then unchanged.
    pub async fn update_classification(&self, ctx: &SecurityContext, classification: Classification) -> StoreResult<()> {
        let event = ChannelUpdateEvent::ClassificationChanged(classification);
        self.update(ctx, &event).await
    }

    /// Has the channel's conversation held in `language` - or in none in
    /// particular - on behalf of `ctx` (`UpdateChannel`, issue 56).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the channel is then unchanged.
    pub async fn update_language(&self, ctx: &SecurityContext, language: Option<Language>) -> StoreResult<()> {
        let event = ChannelUpdateEvent::LanguageChanged(language);
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
        // Back again: no longer a former participant (issue 164).
        channel.former.retain(|f| f.participant.actor_id() != id);
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
    /// and returns it, like [`Self::add_participant`]. It is kept as a
    /// former participant, with who removed it and when (issue 164). The
    /// announcement
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
            .ok_or_else(|| StoreError::NotFound { entity: "participant", id: participant.to_string() })?;
        let removed = channel.participants.remove(index);
        channel.history_limits.retain(|l| l.participant != participant);
        channel.former.retain(|f| f.participant.actor_id() != participant);
        channel.former.push(FormerParticipant { participant: removed.clone(), left: Removal::now(ctx.caller_id) });
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

    /// Stores `channel` on behalf of `ctx`.
    async fn store(&self, ctx: &SecurityContext, channel: &Channel) -> StoreResult<()> {
        self.entity_store.update_channel(ctx, channel).await
    }

    /// Applies `event`, stores the channel, takes it over and announces it.
    async fn update(&self, ctx: &SecurityContext, event: &ChannelUpdateEvent) -> StoreResult<()> {
        let _writing = self.write_lock.lock().await;
        let mut channel = (**self.channel.load()).clone();
        match event {
            ChannelUpdateEvent::NameChanged(name) => channel.name = name.clone(),
            ChannelUpdateEvent::ClassificationChanged(classification) => channel.classification = classification.clone(),
            ChannelUpdateEvent::LanguageChanged(language) => channel.language = language.clone(),
        }
        self.store(ctx, &channel).await?;
        let attributes = channel.security_attributes.clone();
        self.channel.store(Arc::new(channel));
        self.event_publisher.publish(GlobalEvent::ChannelUpdated(self.session_id, self.id, event.clone()), attributes);
        Ok(())
    }
}
