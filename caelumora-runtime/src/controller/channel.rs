use caelumora_model::value::name::Name;
use arc_swap::ArcSwap;
use caelumora_model::value::language::Language;
use caelumora_model::confidentiality::Classification;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::{Channel, ChannelId, ChannelUpdateEvent, FormerParticipant, HistoryLimit, HistoryStart, Participant};
use caelumora_model::removal::Removal;
use caelumora_model::group::Group;
use tracing::error;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::{Grant, SecurityContext, Subject};
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, StoreError, StoreResult};
use std::sync::Arc;
use super::check_restricting;

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
    /// `StoreError::Invalid` if `new_name` is no name, otherwise whatever
    /// the store reports, `StoreError::PermissionDenied` included; the
    /// channel is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: &str) -> StoreResult<()> {
        let event = ChannelUpdateEvent::NameChanged(new_name.to_string());
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
    /// `StoreError::Invalid` if `language` is no language tag, otherwise
    /// whatever the store reports, `StoreError::PermissionDenied` included;
    /// the channel is then unchanged.
    pub async fn update_language(&self, ctx: &SecurityContext, language: Option<&str>) -> StoreResult<()> {
        let event = ChannelUpdateEvent::LanguageChanged(language.map(String::from));
        self.update(ctx, &event).await
    }

    /// Sets whether the channel uses the workspace of its session (issue
    /// 251) on behalf of `ctx` (`UpdateChannel`).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied`
    /// included; the channel is then unchanged.
    pub async fn update_uses_workspace(&self, ctx: &SecurityContext, uses: bool) -> StoreResult<()> {
        self.update(ctx, &ChannelUpdateEvent::UsesWorkspaceChanged(uses)).await
    }

    /// Sets what the channel restricts of the tool calls made in it (issue
    /// 13) on behalf of `ctx` (`UpdateChannel`): `rules` replace those it
    /// had. They apply to the next call of any agent in it.
    ///
    /// # Errors
    /// `StoreError::Validation` if a rule does not only ask or deny
    /// (`ApprovalRule::check_restricting`), otherwise whatever the store
    /// reports; the channel is then unchanged.
    pub async fn update_approval_rules(&self, ctx: &SecurityContext, rules: Vec<ApprovalRule>) -> StoreResult<()> {
        check_restricting(&rules)?;
        self.update(ctx, &ChannelUpdateEvent::ApprovalRulesChanged(rules)).await
    }

    /// Adds `participant` to the channel on behalf of `ctx` (issue 46), seeing
    /// its history from `start` on (`None`: all of it). Stored first
    /// (`UpdateChannel`), then the group of its participants follows, so
    /// that the newcomer may read it (issue 286), then taken over and
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
        self.store(ctx, &channel).await?;
        self.keep_participants(&channel).await;
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
    /// (`AgentChannelLeft` / `UserChannelLeft`) is delivered to whoever was
    /// removed besides the members (issue 23): they are no member any more
    /// when it is delivered.
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
        // Never stored: only who receives the announcement.
        let told = channel.security_attributes.clone().granting(Subject::Actor(participant), Grant::Read);
        self.store(ctx, &channel).await?;
        self.keep_participants(&channel).await;
        self.channel.store(Arc::new(channel));
        if let Some(event) = self.membership_event(participant, false) {
            self.event_publisher.publish(event, told);
        }
        Ok(removed)
    }

    /// Says what the channel is for (issue 245) on behalf of `ctx`
    /// (`UpdateChannel`): `summary` and `briefing` replace what it said - a
    /// missing or empty one removes it. They reach its agents with their
    /// next turn.
    ///
    /// # Errors
    /// `StoreError::Invalid` if one does not fit (`Channel::describe`),
    /// otherwise whatever the store reports; the channel is then unchanged.
    pub async fn update_description(&self, ctx: &SecurityContext, summary: Option<&str>, briefing: Option<&str>) -> StoreResult<()> {
        self.update(ctx, &ChannelUpdateEvent::DescriptionChanged { summary: summary.map(String::from), briefing: briefing.map(String::from) }).await
    }

    /// Announces that agent `agent` left the channel, or came back to it,
    /// without the document changing (issue 314): deleted, it rests in the
    /// channel as a former member; restored, it takes part again. Announced
    /// as a leave or a join is, to whoever reads the channel.
    pub(crate) fn announce_resting(&self, agent: AgentId, resting: bool) {
        let attributes = self.channel.load().security_attributes.clone();
        if let Some(event) = self.membership_event(ActorId::Agent(agent), !resting) {
            self.event_publisher.publish(event, attributes);
        }
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

    /// Lets the group of the participants of `channel` follow them, in the
    /// runtime's name (issue 286): the change of the participants was
    /// permitted and is stored. A failure is logged and corrected by the
    /// next change, which derives the group anew.
    async fn keep_participants(&self, channel: &Channel) {
        if let Err(e) = crate::group::keep_participants(self.entity_store.as_ref(), Group::of_channel(channel)).await {
            error!(channel = %channel.id, "The group of the channel's participants does not follow them: {e}");
        }
    }

    /// Stores `channel` on behalf of `ctx`.
    async fn store(&self, ctx: &SecurityContext, channel: &Channel) -> StoreResult<()> {
        self.entity_store.update_channel(ctx, channel).await
    }

    /// Applies `event`, stores the channel, takes it over and announces it.
    /// What it says is made into the channel's values here (issue 248).
    ///
    /// # Errors
    /// `StoreError::Invalid` if what it says does not fit, otherwise
    /// whatever the store reports.
    async fn update(&self, ctx: &SecurityContext, event: &ChannelUpdateEvent) -> StoreResult<()> {
        let _writing = self.write_lock.lock().await;
        let mut channel = (**self.channel.load()).clone();
        match event {
            ChannelUpdateEvent::NameChanged(name) => channel.name = Name::new(name)?,
            ChannelUpdateEvent::ClassificationChanged(classification) => channel.classification = classification.clone(),
            ChannelUpdateEvent::LanguageChanged(language) => channel.language = Language::optional(language.as_deref())?,
            ChannelUpdateEvent::ApprovalRulesChanged(rules) => channel.approval_rules = rules.clone(),
            ChannelUpdateEvent::DescriptionChanged { summary, briefing } => channel.describe(summary.as_deref(), briefing.as_deref())?,
            ChannelUpdateEvent::UsesWorkspaceChanged(uses) => channel.uses_workspace = *uses,
        }
        self.store(ctx, &channel).await?;
        let attributes = channel.security_attributes.clone();
        self.channel.store(Arc::new(channel));
        self.event_publisher.publish(GlobalEvent::ChannelUpdated(self.session_id, self.id, event.clone()), attributes);
        Ok(())
    }
}
