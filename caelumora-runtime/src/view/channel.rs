//! A channel as its viewer reached it, and its members.

use caelumora_model::store::query::Query;
use super::member::{member_state, AgentMember, MemberState};
use super::{AgentContextView, SessionView, ViewError, ViewResult, Views, WorkspaceView};
use crate::channel::{visible_part, ChannelLoop, VisibleHistory};
use caelumora_model::message_history::HistoryEntry;
use crate::moderator::Moderator;
use crate::moderator::ResponderPolicy;
use caelumora_model::actor::{ActorId, ActorState};
use caelumora_model::approval::ApprovalRequest;
use caelumora_model::command::Command;
use caelumora_model::component::ComponentConfig;
use caelumora_model::channel::{Channel, ChannelState, Participant};
use caelumora_model::value::text::{Briefing, Summary};
use caelumora_model::confidentiality::Classification;
use caelumora_model::value::language::Language;
use caelumora_model::digest::Digest;
use caelumora_model::agent::{AgentChannelState, PersonalityKind};
use caelumora_model::value::name::Name;
use caelumora_model::note::Note;
use caelumora_model::removal::Removal;
use caelumora_model::work::TurnId;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, Place, SecurityContext};
use caelumora_model::session::ChannelId;
use caelumora_model::snapshot::{ChannelSnapshot, Detail, MemberSnapshot, PublishedNotes};
use caelumora_model::store::StoreError;
use caelumora_model::tool::ToolSet;
use std::sync::Arc;
use tracing::warn;

/// How much of a channel its viewer may read (issue 133).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Readable {
    /// All of it.
    Whole,
    /// Only what was released to where the viewer reads from: entry by
    /// entry, each by its own classification. A channel of which nothing
    /// is readable is answered as one that does not exist.
    Released,
}

/// A channel the viewer may read.
#[derive(Debug, Clone)]
pub struct ChannelView {
    session: SessionView,
    /// The document as it was when the channel was reached.
    channel: Arc<Channel>,
    /// Its loop, if it runs.
    running: Option<Arc<ChannelLoop>>,
    readable: Readable,
}

impl ChannelView {
    pub(super) fn of(session: SessionView, channel: Arc<Channel>, running: Option<Arc<ChannelLoop>>, readable: Readable) -> Self {
        Self { session, channel, running, readable }
    }

    fn views(&self) -> &Views {
        self.session.views()
    }

    /// The session the channel belongs to.
    pub fn session(&self) -> &SessionView {
        &self.session
    }

    /// The workspace of the session, if the viewer may use it in this
    /// channel (issue 251, decision 10): as a current member, the channel
    /// using it, and it and the channel flowing both ways. Nothing
    /// otherwise - also not why, nor that there is one.
    ///
    /// # Errors
    /// `Failed` if reading the workspace failed.
    pub async fn workspace(&self) -> ViewResult<Option<WorkspaceView>> {
        let usable = self.views().workspace_manager().usable_in(self.views().context(), self.session.as_session(), &self.channel).await
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("reading the session's workspace")))?;
        Ok(usable.map(|workspace| WorkspaceView::in_channel(self.views().clone(), workspace, self.session.id(), self.channel.id)))
    }

    /// The artifacts with a version published in this channel, each with
    /// those versions alone that the viewer may read (issues 271, 23), if
    /// they read the channel as a whole; none otherwise. Their content is
    /// reached by the artifact, never through the channel.
    ///
    /// # Errors
    /// What the artifacts' manager reports.
    pub async fn artifacts(&self) -> ViewResult<Vec<super::ArtifactView>> {
        if self.readable != Readable::Whole {
            return Ok(Vec::new());
        }
        use caelumora_model::artifact::ArtifactField;
        use caelumora_model::store::query::Filter;
        let failed = |e: caelumora_model::store::StoreError| ViewError::Failed(anyhow::Error::new(e).context("listing the artifacts"));
        let (session, channel) = (self.session.id(), self.channel.id);
        // Those with a version published here, which the store is asked for
        // alone (issue 293).
        let here = Filter::And(vec![
            Filter::eq(ArtifactField::VersionSession, &session).map_err(failed)?,
            Filter::eq(ArtifactField::VersionChannel, &channel).map_err(failed)?,
        ]);
        let all = self.views().entities().list_artifacts(&SecurityContext::system(), &Query::filtered(Filter::any(ArtifactField::Versions, here))).await.map_err(failed)?.items;
        let mut published = Vec::new();
        for mut artifact in all {
            artifact.versions.retain(|version| version.session_id == session && version.channel_id == channel);
            if artifact.versions.is_empty() {
                continue;
            }
            if let Some(view) = self.views().readable_artifact(artifact).await {
                published.push(view);
            }
        }
        Ok(published)
    }

    pub fn id(&self) -> ChannelId {
        self.channel.id
    }

    pub fn name(&self) -> &Name {
        &self.channel.name
    }

    /// The kind of the channel's moderator, e.g. `policy_moderator` (issue
    /// 174); `None` if it is a preconfigured component.
    pub fn moderator(&self) -> Option<String> {
        self.views().components().kind_of::<dyn Moderator>(&self.channel.components)
    }

    /// The kind of the policy configured for the moderator's slot, e.g.
    /// `round_robin_policy` (issues 44, 174); `None` if it is a
    /// preconfigured component.
    pub fn responder_policy(&self) -> Option<String> {
        self.views().components().kind_of::<dyn ResponderPolicy>(&self.channel.components)
    }

    /// The classification of the channel's contents (issue 133).
    pub fn classification(&self) -> &Classification {
        &self.channel.classification
    }

    /// The language the channel's conversation is held in (issue 56), if
    /// any.
    pub fn language(&self) -> Option<&Language> {
        self.channel.language.as_ref()
    }

    /// What the channel is for, in a line (issue 245); `None` if nobody said.
    pub fn summary(&self) -> Option<&Summary> {
        self.channel.summary.as_ref()
    }

    /// What an agent should know before it works in the channel (issue 245);
    /// `None` if nobody wrote it.
    pub fn briefing(&self) -> Option<&Briefing> {
        self.channel.briefing.as_ref()
    }

    /// What the channel holds classified `classification`: one side of the
    /// question whether it may be read from elsewhere.
    pub fn place(&self, classification: &Classification) -> Place {
        Place { channel: self.channel.id, classification: classification.clone(), session: self.session.privacy() }
    }

    /// Whether the channel runs: its loop is where an operation goes.
    pub fn running(&self) -> Option<&Arc<ChannelLoop>> {
        self.running.as_ref()
    }

    /// Whether the viewer may read an entry of the channel classified
    /// `classification`: every one if it reads the channel as a whole, else
    /// those released to where it reads from.
    pub(super) async fn may_read_entry(&self, classification: &Classification) -> bool {
        match self.readable {
            Readable::Whole => true,
            Readable::Released => self.views().may_copy_from(&self.place(classification)).await,
        }
    }

    /// What a read of a channel read entry by entry answers when nothing
    /// could be read: that there is no such channel, so that nothing can be
    /// probed.
    fn nothing_released(&self) -> ViewError {
        ViewError::NoChannelNamed(self.channel.name.to_string())
    }

    /// Whether the viewer may rename the channel and add or remove its
    /// participants (`UpdateChannel`, issue 46).
    pub async fn may_change(&self) -> bool {
        self.views().security().may_change_channel(self.views().context(), &self.channel.security_attributes).await
    }

    /// Its participants, in the order of the channel.
    pub async fn members(&self) -> Vec<MemberView> {
        let mut members = Vec::with_capacity(self.channel.participants.len());
        for participant in &self.channel.participants {
            members.push(MemberView::of(self.clone(), participant.clone()).await);
        }
        members
    }

    /// Who took part and left (issue 164), in the order they left: their
    /// messages are in the history and an agent's state in the store, so
    /// what it saw stays readable.
    pub async fn former_members(&self) -> Vec<MemberView> {
        let mut former = Vec::with_capacity(self.channel.former.len());
        for left in &self.channel.former {
            former.push(MemberView::of_former(self.clone(), left.participant.clone(), Some(left.left.clone())).await);
        }
        former
    }

    /// The member `actor` of the channel - or one that took part and left,
    /// a former member (`MemberView::former`, issues 163, 164). An agent
    /// that left before the channel kept who left is known by the messages
    /// it left in the history the viewer sees.
    ///
    /// # Errors
    /// `MemberNotFound` if `actor` neither takes part in the channel nor
    /// left it; `Failed` if the history cannot be read.
    pub async fn member(&self, actor: ActorId) -> ViewResult<MemberView> {
        if let Some(participant) = self.channel.participants.iter().find(|p| p.actor_id() == actor) {
            return Ok(MemberView::of(self.clone(), participant.clone()).await);
        }
        if let Some(left) = self.channel.former.iter().find(|f| f.participant.actor_id() == actor) {
            return Ok(MemberView::of_former(self.clone(), left.participant.clone(), Some(left.left.clone())).await);
        }
        let not_found = || ViewError::MemberNotFound { channel: self.channel.id, actor };
        let ActorId::Agent(agent) = actor else { return Err(not_found()) };
        if !self.history().await?.messages().iter().any(|m| m.sender_id == actor) {
            return Err(not_found());
        }
        Ok(MemberView::of_former(self.clone(), Participant::Agent(agent), None).await)
    }

    /// The history as the viewer sees it (issue 46): all of it, unless they
    /// joined later with a limit - with what a reader records as read
    /// (issue 72); of a channel read entry by entry, the messages released
    /// to where the viewer reads from. From the running loop, or from the
    /// store.
    ///
    /// # Errors
    /// `Failed` if the stored history cannot be read; `NoChannelNamed` if
    /// nothing of a channel read entry by entry can be.
    pub async fn history(&self) -> ViewResult<VisibleHistory> {
        // Whoever reads: an agent reads as itself, not as the owner it acts
        // for - its own limit applies, not the owner's.
        let actor = self.views().viewer().context().caller_id;
        let mut history = match &self.running {
            Some(channel_loop) => channel_loop.visible_history(actor).await,
            None => {
                let state = match self.views().states().get_channel_state(self.channel.id).await {
                    Ok(state) => state,
                    // Nothing was said yet.
                    Err(StoreError::NotFound { .. }) => ChannelState::new(self.channel.id),
                    Err(e) => {
                        warn!("Cannot read the history of channel {}: {}", self.channel.id, e);
                        return Err(ViewError::Failed(anyhow::Error::new(e).context(format!("reading the history of channel {}", self.channel.id))));
                    }
                };
                let start = self.channel.history_limits.iter().find(|l| l.participant == actor).map(|l| l.start);
                VisibleHistory { items: visible_part(start, state.history.items()), digest: state.history.digest(), start }
            }
        };
        if self.readable == Readable::Released {
            // The messages released, and nothing of how turns ended: that
            // would tell of work in the channel whose messages are not
            // released (issue 258).
            let mut released = Vec::with_capacity(history.items.len());
            for item in history.items {
                if let HistoryEntry::Said { message, .. } = &item {
                    if self.may_read_entry(&message.classification).await {
                        released.push(item);
                    }
                }
            }
            if released.is_empty() {
                return Err(self.nothing_released());
            }
            history.items = released;
        }
        Ok(history)
    }

    /// The notes every agent of the channel published (issue 17), read from
    /// their states in the store: notes of finished turns only; of a
    /// channel read entry by entry, those released to where the viewer
    /// reads from.
    ///
    /// # Errors
    /// `Failed` if an agent's state cannot be read; `NoChannelNamed` if
    /// nothing of a channel read entry by entry can be.
    pub async fn notes(&self) -> ViewResult<Vec<PublishedNotes>> {
        let mut published = Vec::new();
        for member in self.members().await {
            if let Some(notes) = member.notes().await? {
                published.push(notes);
            }
        }
        if self.readable == Readable::Released && published.iter().all(|p| p.notes.is_empty()) {
            return Err(self.nothing_released());
        }
        Ok(published)
    }

    /// The approvals the channel's agents wait for (issue 172), read from
    /// their turns as stored - the record the approver is asked again from
    /// after a restart - for a viewer who takes part in the channel as a
    /// user: the users decide, and are told of every request as it is made.
    /// None for any other viewer.
    ///
    /// # Errors
    /// `Failed` if an agent's state cannot be read.
    pub async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>> {
        if !self.shows_approvals() {
            return Ok(Vec::new());
        }
        let mut pending = Vec::new();
        for member in self.members().await {
            pending.extend(member.pending_approvals().await?);
        }
        Ok(pending)
    }

    /// Whether the viewer is shown the approvals the channel's agents wait
    /// for: as a user who takes part in it - the users decide - and reads
    /// all of it. Who looks, not whose rights they have: an agent is never
    /// shown its owner's (issue 255).
    fn shows_approvals(&self) -> bool {
        let viewer = self.views().viewer().context().caller_id;
        let takes_part = self.channel.participants.iter().any(|p| matches!(p, Participant::User(_)) && p.actor_id() == viewer);
        takes_part && self.readable == Readable::Whole
    }

    /// The channel as the viewer sees it at this moment (issue 80), with
    /// the details `detail` asks for. A detail that cannot be read is left
    /// empty: the channel can be shown without it.
    /// The session's workspace, if the channel uses it and the viewer may
    /// read it as its access list lets them (issue 23).
    async fn readable_workspace(&self) -> Option<caelumora_model::workspace::WorkspaceId> {
        let id = self.session.as_session().workspace.filter(|_| self.channel.uses_workspace)?;
        self.views().workspace(id).await.ok().map(|workspace| workspace.id())
    }

    pub async fn snapshot(&self, detail: Detail) -> ChannelSnapshot {
        let (mut members, mut former_members) = (Vec::new(), Vec::new());
        if detail.contains(Detail::MEMBERS) {
            for member in self.members().await {
                members.push(member.snapshot(detail).await);
            }
            for member in self.former_members().await {
                former_members.push(member.snapshot(Detail::empty()).await);
            }
        }
        ChannelSnapshot {
            id: self.channel.id,
            name: self.channel.name.to_string(),
            summary: self.channel.summary.clone(),
            briefing: self.channel.briefing.clone().filter(|_| detail.contains(Detail::BRIEFING)),
            moderator: self.moderator(),
            responder_policy: self.responder_policy(),
            classification: self.channel.classification.clone(),
            language: self.channel.language.clone(),
            approval_rules: self.channel.approval_rules.clone(),
            uses_workspace: self.channel.uses_workspace,
            workspace_id: self.readable_workspace().await,
            can_change: self.may_change().await,
            members,
            former_members,
        }
    }
}

/// A participant of a channel - a user, an agent, an approver - as the
/// viewer reached it. What differs between the kinds is its `MemberState`;
/// what the viewer may see of it is decided here, for every kind alike.
#[derive(Debug, Clone)]
pub struct MemberView {
    channel: ChannelView,
    participant: Participant,
    /// Took part once and left.
    former: bool,
    /// Who removed it and when, if the channel kept it (issue 164).
    left: Option<Removal>,
    /// What the member is, as its kind makes it.
    state: Arc<dyn MemberState>,
}

impl MemberView {
    /// `participant`, who takes part in `channel`.
    async fn of(channel: ChannelView, participant: Participant) -> Self {
        let state = member_state(channel.views(), &participant, channel.id()).await;
        Self { channel, participant, former: false, left: None, state }
    }

    /// `participant`, who took part in `channel` and left - removed as
    /// `left` says, if the channel kept it.
    async fn of_former(channel: ChannelView, participant: Participant, left: Option<Removal>) -> Self {
        let state = member_state(channel.views(), &participant, channel.id()).await;
        Self { channel, participant, former: true, left, state }
    }

    /// Whether the member took part once and left: not among the channel's
    /// participants any more, but what it said and saw is still there.
    pub fn former(&self) -> bool {
        self.former
    }

    /// Who removed the member and when (issue 164); `None` for one that
    /// takes part, and for one that left before the channel kept it.
    pub fn left(&self) -> Option<&Removal> {
        self.left.as_ref()
    }

    /// The channel the member takes part in.
    pub fn channel(&self) -> &ChannelView {
        &self.channel
    }

    /// The views the member was reached through: in whose name it is read.
    fn views(&self) -> &Views {
        self.channel.views()
    }

    pub fn id(&self) -> ActorId {
        self.participant.actor_id()
    }

    /// `None` for a user or agent that no longer exists.
    pub fn name(&self) -> Option<&str> {
        self.state.name()
    }

    /// The handle the member is `@`-mentioned by (see `crate::mention`), or
    /// `None` if it cannot be mentioned - approvers, participants that no
    /// longer exist, names that yield no handle.
    pub fn handle(&self) -> Option<String> {
        self.state.handle()
    }

    /// How the member is introduced (issue 176): an agent as its
    /// personality introduces it, a user as they introduce themselves in
    /// their profile. `None` for an approver, for a member that has no
    /// introduction, and for one that no longer exists. Public, like the
    /// name.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    pub async fn introduction(&self) -> ViewResult<Option<String>> {
        self.state.introduction().await
    }

    /// The name of an agent member's personality, without its scope, and
    /// whether the agent runs here or elsewhere (issue 244). Public, like
    /// its introduction - not the personality itself, which stays its
    /// readers'. `None` for a member that is no agent and for an agent
    /// whose personality is gone.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    pub async fn personality(&self) -> ViewResult<Option<(String, PersonalityKind)>> {
        Ok(self.state.personality().await?.map(|personality| (personality.name.to_string(), personality.kind)))
    }

    /// Who owns an agent member (issue 244): whose authority it acts with
    /// and who answers for its tool calls. `None` for a member that is no
    /// agent or no longer exists.
    pub fn owner(&self) -> Option<ActorId> {
        self.state.attributes().map(|attributes| attributes.owner_id)
    }

    /// Whether the viewer may see what this member saw when it produced a
    /// message (`ReadAgentContext`, issue 41) - only ever for agents.
    pub async fn may_read_context(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) => self.views().security().may_read_agent_context(self.views().context(), attributes).await,
            None => false,
        }
    }

    /// Whether the viewer may see which tools the member has now
    /// (`ReadAgentTools`, issue 194) - only ever for agents.
    pub async fn may_read_tools(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) => self.views().security().may_read_agent_tools(self.views().context(), attributes, &self.channel.channel.security_attributes).await,
            None => false,
        }
    }

    /// Whether the viewer may see which components the member was built
    /// from (`ReadAgentComponents`, issue 270) - only ever for agents.
    pub async fn may_read_components(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) => self.views().security().may_read_agent_components(self.views().context(), attributes).await,
            None => false,
        }
    }

    /// Whether the viewer may approve the tool calls of the member
    /// (`ApproveToolCall`, issue 14) - only ever for agents taking part.
    /// Who may is asked when it calls a tool needing approval; everybody
    /// else sees only that it waits.
    pub async fn may_approve(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) if !self.former => self.views().security().may_approve(self.views().context(), attributes, &self.channel.channel.security_attributes).await,
            _ => false,
        }
    }

    /// Whether the viewer may ask the member to answer again instead of one
    /// of its messages (`RetryMessage`, issues 234, 242) - only ever an
    /// agent taking part that answers again, not one that runs elsewhere.
    /// Whether it does is the agent's to decide. If its kind cannot be
    /// read, it is not offered; nor if its personality is gone: then it is
    /// not run (issue 189).
    pub async fn may_retry(&self) -> bool {
        if self.former {
            return false;
        }
        match self.state.personality().await {
            Ok(Some(personality)) if personality.kind.can_retry() => {
                self.views().security().may_retry_message(self.views().context(), &self.channel.channel.security_attributes).await
            }
            Ok(_) => false,
            Err(e) => {
                warn!("Reading whether {} can retry failed: {:#}", self.id(), e);
                false
            }
        }
    }

    /// Whether the viewer may end the member's answer under way
    /// (`CancelChannel`, issues 235, 243) - only ever an agent taking part,
    /// of either kind.
    pub async fn may_cancel(&self) -> bool {
        match self.state.agent() {
            Some(_) if !self.former => self.views().security().may_cancel_channel(self.views().context(), &self.channel.channel.security_attributes).await,
            _ => false,
        }
    }

    /// Where the member stands now (issue 243) - only ever an agent taking
    /// part, read from the running channel; none in a channel that does not
    /// run.
    pub fn state(&self) -> Option<ActorState> {
        self.state.agent()?;
        self.channel.running().filter(|_| !self.former)?.actor_by_id(self.id()).map(|actor| actor.state())
    }

    /// The commands the member carries out itself that the viewer may see
    /// (issue 269), read from its actor, which asks the policy: none if it
    /// carries out none here - it is no agent, has left - and none while the
    /// channel does not run, when nothing could carry them out.
    pub async fn commands(&self) -> Vec<Command> {
        if self.former {
            return Vec::new();
        }
        let Some(actor) = self.channel.running().and_then(|channel| channel.actor_by_id(self.id())) else { return Vec::new() };
        actor.commands(self.views().context()).await
    }

    /// The tools the member has now, in their tool sets (issue 194), as it
    /// would offer them at the start of its next turn - read from its
    /// actor, since an agent's tools are built for each of its channels.
    /// None if it has none here: it is no agent run here or has left - and
    /// also while the channel does not run, when its tools are not built.
    ///
    /// # Errors
    /// `Forbidden` if the member is an agent whose tools the viewer may not
    /// see (`ReadAgentTools`).
    pub async fn tool_sets(&self) -> ViewResult<Vec<ToolSet>> {
        let Some(attributes) = self.state.attributes() else { return Ok(Vec::new()) };
        let views = self.views();
        let forbidden = || ViewError::Forbidden { actor: self.id() };
        views.security()
            .require(views.context(), ActionType::ReadAgentTools { agent: attributes, channel: &self.channel.channel.security_attributes }, forbidden).await
            .map_err(|_| forbidden())?;
        if self.former {
            return Ok(Vec::new());
        }
        let Some(actor) = self.channel.running().and_then(|channel| channel.actor_by_id(self.id())) else { return Ok(Vec::new()) };
        Ok(actor.tool_sets().await)
    }

    /// The components the member was built from in this channel, the
    /// preconfigured ones it includes resolved (issue 270) - read from its
    /// actor, which recorded them when it was built: what it is, not what
    /// its personality would make of it now. None if it is no agent run
    /// here or has left - and also while the channel does not run.
    ///
    /// # Errors
    /// `Forbidden` if the member is an agent whose components the viewer
    /// may not see (`ReadAgentComponents`).
    pub async fn components(&self) -> ViewResult<ComponentConfig> {
        let Some(attributes) = self.state.attributes() else { return Ok(ComponentConfig::default()) };
        let views = self.views();
        let forbidden = || ViewError::Forbidden { actor: self.id() };
        views.security()
            .require(views.context(), ActionType::ReadAgentComponents(attributes), forbidden).await
            .map_err(|_| forbidden())?;
        if self.former {
            return Ok(ComponentConfig::default());
        }
        let Some(actor) = self.channel.running().and_then(|channel| channel.actor_by_id(self.id())) else { return Ok(ComponentConfig::default()) };
        Ok(actor.components())
    }

    /// Whether the viewer may delete the member - an agent that exists,
    /// its owner (`DeleteAgent`, issue 164).
    pub async fn may_delete(&self) -> bool {
        match (self.state.agent(), self.state.attributes()) {
            (Some(_), Some(attributes)) => self.views().security().may_delete_agent(self.views().context(), attributes).await,
            _ => false,
        }
    }

    /// The notes the member published in the channel (issue 17), with the
    /// digest of their chain - of a channel read entry by entry, those
    /// released to where the viewer reads from; `None` for a member that
    /// is no agent.
    ///
    /// # Errors
    /// `Failed` if the agent's state cannot be read.
    pub async fn notes(&self) -> ViewResult<Option<PublishedNotes>> {
        let Some(agent) = self.state.agent() else { return Ok(None) };
        let state = agent.stored().await?;
        let notes = self.notes_in(&state).await;
        Ok(Some(PublishedNotes { agent: self.id(), name: self.name().unwrap_or_default().to_string(), notes, digest: state.notes.digest() }))
    }

    /// Of the notes the agent published, as its `state` holds them, those
    /// the viewer may read.
    async fn notes_in(&self, state: &AgentChannelState) -> Vec<Note> {
        self.channel.released(state.notes.current()).await
    }

    /// The approvals the member's turn waits for (issue 172), as stored;
    /// none for a member that is no agent or has left.
    ///
    /// # Errors
    /// `Failed` if the agent's state cannot be read.
    async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>> {
        match self.state.agent() {
            Some(agent) if !self.former => Ok(agent.stored().await?.pending_approvals()),
            _ => Ok(Vec::new()),
        }
    }

    /// The private notes the member wrote in the channel (issue 51), with
    /// the digest of their chain, for a viewer who may read what it saw
    /// (`ReadAgentContext`) - of a channel read entry by entry, those
    /// released to where the viewer reads from (issue 133).
    ///
    /// # Errors
    /// `NoContext` if the member is no agent, `Forbidden` if the viewer may
    /// not read what it saw, `Failed` if its state cannot be read.
    pub async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        let agent = self.readable_agent().await?;
        let (notes, digest) = agent.private_notes().await?;
        Ok((self.channel.released(notes).await, digest))
    }

    /// What the member saw in the channel (issue 41): its transcript, its
    /// private notes, the context of each of its messages, the turns it
    /// gave up - of an external agent, what the remote agent was given
    /// (issue 179) - for a viewer who may read its context
    /// (`ReadAgentContext`) and the channel as a whole.
    ///
    /// # Errors
    /// `NoContext` if the member is no agent, `Forbidden` if the viewer may
    /// not read what it saw or reads the channel entry by entry only,
    /// `Failed` if its state cannot be read.
    pub async fn context(&self) -> ViewResult<AgentContextView> {
        let agent = self.readable_agent().await?;
        if self.channel.readable == Readable::Released {
            return Err(ViewError::Forbidden { actor: self.id() });
        }
        agent.context().await
    }

    /// The member as an agent whose context the viewer may read.
    async fn readable_agent(&self) -> ViewResult<&AgentMember> {
        let no_context = || ViewError::NoContext { actor: self.id() };
        let attributes = self.state.attributes().ok_or_else(no_context)?;
        let views = self.views();
        let forbidden = || ViewError::Forbidden { actor: self.id() };
        views.security()
            .require(views.context(), ActionType::ReadAgentContext(attributes), forbidden).await
            .map_err(|_| forbidden())?;
        self.state.agent().ok_or_else(no_context)
    }

    /// The turn of the answer the member is giving now (issue 240), from the
    /// running channel - none of a former member, nor in a channel that does
    /// not run.
    fn turn_under_way(&self) -> Option<TurnId> {
        self.channel.running().filter(|_| !self.former)?.turn_under_way(self.id())
    }

    /// The member as the viewer sees it at this moment (issue 80), with
    /// the details `detail` asks for - those of an agent taken from its
    /// state, read once for all of them; who it is (`PROFILE`) from its
    /// personality, its owner and a user's profile. What cannot be read is
    /// left out.
    pub async fn snapshot(&self, detail: Detail) -> MemberSnapshot {
        let mut wanted = detail & (Detail::USAGE | Detail::NOTES | Detail::APPROVALS);
        if self.former || !self.channel.shows_approvals() {
            wanted.remove(Detail::APPROVALS);
        }
        let state = match self.state.agent() {
            Some(agent) if !wanted.is_empty() => agent.stored().await
                .inspect_err(|e| warn!("Reading the state of {} failed: {:#}", self.id(), e))
                .ok(),
            _ => None,
        };
        let (mut introduction, mut personality, mut owner, mut owner_name) = (None, None, None, None);
        if detail.contains(Detail::PROFILE) {
            introduction = self.introduction().await
                .inspect_err(|e| warn!("Reading the introduction of {} failed: {:#}", self.id(), e))
                .ok().flatten();
            personality = self.personality().await
                .inspect_err(|e| warn!("Reading the personality of {} failed: {:#}", self.id(), e))
                .ok().flatten();
            owner = self.owner();
            if let Some(owner) = owner {
                owner_name = self.views().name_of(owner).await;
            }
        }
        let (personality, personality_kind) = personality.unzip();
        let (mut usage, mut notes, mut pending_approvals, mut pending_questions) = (None, Vec::new(), Vec::new(), Vec::new());
        if let Some(state) = &state {
            if wanted.contains(Detail::USAGE) {
                usage = state.usage(self.id());
            }
            if wanted.contains(Detail::NOTES) {
                notes = self.notes_in(state).await;
            }
            if wanted.contains(Detail::APPROVALS) {
                pending_approvals = state.pending_approvals();
                // Sensitive questions only for those asked (issue 165) -
                // in person, never to an agent of theirs.
                let viewer = self.views().viewer().context().caller_id;
                pending_questions = state.pending_questions().into_iter()
                    .filter(|q| q.shown_to().is_none_or(|asked| asked.contains(&viewer)))
                    .collect();
            }
        }
        MemberSnapshot {
            id: self.id(),
            name: self.name().map(str::to_string),
            handle: self.handle(),
            can_read_context: self.may_read_context().await,
            can_read_tools: self.may_read_tools().await,
            can_read_components: self.may_read_components().await,
            can_approve: self.may_approve().await,
            can_retry: self.may_retry().await,
            can_cancel: self.may_cancel().await,
            can_delete: self.may_delete().await,
            turn_under_way: self.turn_under_way(),
            state: self.state(),
            commands: self.commands().await,
            usage,
            notes,
            pending_approvals,
            pending_questions,
            left: self.left.clone(),
            introduction,
            personality,
            personality_kind,
            owner,
            owner_name,
        }
    }
}

impl ChannelView {
    /// Of `notes`, those the viewer may read (`may_read_entry`).
    async fn released(&self, notes: Vec<Note>) -> Vec<Note> {
        if self.readable == Readable::Whole {
            return notes;
        }
        let mut released = Vec::with_capacity(notes.len());
        for note in notes {
            if self.may_read_entry(&note.classification).await {
                released.push(note);
            }
        }
        released
    }
}
