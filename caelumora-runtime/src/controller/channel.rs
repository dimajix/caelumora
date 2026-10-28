use caelumora_model::value::name::Name;
use arc_swap::Guard;
use caelumora_model::confidentiality::Classification;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::{self, Channel, ChannelId, ChannelUpdateEvent, FormerParticipant, Participant};
use caelumora_model::removal::Removal;
use caelumora_model::message::Message;
use caelumora_model::group::Group;
use tracing::error;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::{SecurityContext, Subject};
use caelumora_model::store::{EntityStore, EntityStores, StoreError, StoreResult};
use std::sync::Arc;
use super::check_restricting;
use super::document::LiveDocument;
use crate::group::keep_participants;

/// The one writer of a channel's document (issue 70): changes are stored
/// first - where the permission is checked, on the channel's own attributes
/// - and only then taken over live and announced.
#[derive(Debug)]
pub struct ChannelController {
    channel: LiveDocument<Channel>,
    groups: Arc<dyn EntityStore<Group>>,
}

impl ChannelController {
    pub fn new(channel: Channel, event_publisher: Arc<dyn EventPublisher>, stores: &EntityStores) -> Self {
        Self {
            channel: LiveDocument::new((channel.session_id, channel.id), channel, Arc::clone(&stores.channels), event_publisher),
            groups: Arc::clone(&stores.groups),
        }
    }

    pub fn load(&self) -> Guard<Arc<Channel>> {
        self.channel.load()
    }

    pub fn id(&self) -> ChannelId {
        self.channel.address().1
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

    /// Adds `participant` to the channel on behalf of `ctx` (issue 46). How
    /// much of the history it is given is the history's to record
    /// (`HistoryShared`, issue 417). Stored first
    /// (`UpdateChannel`), then the group of its participants follows, so
    /// that the newcomer may read it (issue 286), then taken over and
    /// announced (`channel::JOINED`).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the channel is then unchanged.
    pub async fn add_participant(&self, ctx: &SecurityContext, participant: Participant) -> StoreResult<()> {
        let writing = self.channel.write().await;
        let mut channel = writing.current();
        let id = participant.actor_id();
        channel.participants.push(participant);
        // Back again: no longer a former participant (issue 164).
        channel.former.retain(|f| f.participant.actor_id() != id);
        let channel = writing.store(ctx, channel).await?;
        self.keep_participants(&channel).await;
        if let Some(event) = self.joined_or_left_event(id, true) {
            writing.announce(event, &channel);
        }
        Ok(())
    }

    /// Removes `participant` from the channel on behalf of `ctx` (issue 46)
    /// and returns it, like [`Self::add_participant`]. It is kept as a
    /// former participant, with who removed it and when (issue 164). The
    /// announcement (`channel::LEFT`) is delivered to whoever was
    /// removed besides the participants (issue 23): they are no participant any more
    /// when it is delivered.
    ///
    /// # Errors
    /// `StoreError::NotFound` if `participant` takes no part in the channel;
    /// whatever the store reports otherwise.
    pub async fn remove_participant(&self, ctx: &SecurityContext, participant: ActorId) -> StoreResult<Participant> {
        let writing = self.channel.write().await;
        let mut channel = writing.current();
        let index = channel.participants.iter().position(|p| p.actor_id() == participant)
            .ok_or_else(|| StoreError::NotFound { entity: "participant", id: participant.to_string() })?;
        let removed = channel.participants.remove(index);
        channel.former.retain(|f| f.participant.actor_id() != participant);
        channel.former.push(FormerParticipant { participant: removed.clone(), left: Removal::now(ctx.caller_id) });
        let channel = writing.store(ctx, channel).await?;
        self.keep_participants(&channel).await;
        if let Some(event) = self.joined_or_left_event(participant, false) {
            writing.announce_also_to(event, &channel, [Subject::Actor(participant)]);
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
    /// channel as a former participant; restored, it takes part again. Announced
    /// as a leave or a join is, to whoever reads the channel.
    pub(crate) fn announce_resting(&self, agent: AgentId, resting: bool) {
        if let Some(event) = self.joined_or_left_event(ActorId::Agent(agent), !resting) {
            self.channel.announce(event);
        }
    }

    /// Announces that `message` was said in the channel and is stored (issue
    /// 404): to whoever reads the channel, as it is classified - who would
    /// read the message in its history.
    pub(crate) fn announce_said(&self, message: &Message) {
        self.channel.announce(GlobalEvent::custom::<Channel>(self.channel.address(), channel::SAID, message));
    }

    /// The event announcing that `participant` joined or left.
    fn joined_or_left_event(&self, participant: ActorId, joined: bool) -> Option<GlobalEvent> {
        let name = if joined { channel::JOINED } else { channel::LEFT };
        match participant {
            ActorId::Agent(_) | ActorId::User(_) => Some(GlobalEvent::custom::<Channel>(self.channel.address(), name, &participant)),
            _ => None,
        }
    }

    /// Lets the group of the participants of `channel` follow them, in the
    /// runtime's name (issue 286): the change of the participants was
    /// permitted and is stored. A failure is logged and corrected by the
    /// next change, which derives the group anew.
    async fn keep_participants(&self, channel: &Channel) {
        if let Err(e) = keep_participants(self.groups.as_ref(), Group::of_channel(channel)).await {
            error!(channel = %channel.id, "The group of the channel's participants does not follow them: {e}");
        }
    }

    /// Applies `event` on behalf of `ctx` (`LiveDocument::update`): stored,
    /// taken over and announced - to who reached the channel as it was,
    /// too, so that who no longer does learns so (issue 312).
    ///
    /// # Errors
    /// `StoreError::Invalid` if what it says does not fit, otherwise
    /// whatever the store reports.
    async fn update(&self, ctx: &SecurityContext, event: &ChannelUpdateEvent) -> StoreResult<()> {
        self.channel.update(ctx, event).await.map(drop)
    }
}
