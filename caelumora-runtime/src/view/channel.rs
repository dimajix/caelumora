//! A channel as its viewer reached it, and its participants.

use super::participant::{participant_state, AgentParticipant, ParticipantState};
use super::{AgentContextView, SessionView, ViewError, ViewResult, Views, WorkspaceView};
use crate::actor::user::ChannelWatch;
use crate::attachment::AttachmentError;
use crate::terminal::ByteStream;
use crate::channel::ChannelLoop;
use crate::error::component_message;
use crate::session::ChannelRun;
use crate::moderator::Moderator;
use crate::moderator::ResponderPolicy;
use caelumora_model::chain::Position;
use caelumora_model::actor::{ActorDescription, ActorId, ActorState};
use caelumora_model::agent::{AgentChannelState, PersonalityKind};
use caelumora_model::approval::ApprovalRequest;
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::command::Command;
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::Classification;
use caelumora_model::content::{ContentId, ContentRef};
use caelumora_model::confidentiality::{Reading, Place};
use caelumora_model::digest::Digest;
use caelumora_model::history::{HistoryEntry, HistoryPage, Reader};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::note::Note;
use caelumora_model::removal::Removal;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement};
use caelumora_model::session::ChannelId;
use caelumora_model::snapshot::{ChannelSnapshot, Detail, ChannelParticipantSnapshot, PublishedNotes, Unread};
use caelumora_model::store::history::HistoryField;
use caelumora_model::store::query::Filter;
use caelumora_model::store::{StoreError, StoreResultExt};
use caelumora_model::tool::ToolSet;
use caelumora_model::value::language::Language;
use caelumora_model::value::name::Name;
use caelumora_model::value::text::{Briefing, Introduction, Summary};
use caelumora_model::work::TurnId;
use std::sync::Arc;
use tracing::warn;
use caelumora_model::workspace::WorkspaceId;

/// A channel the viewer may read.
#[derive(Debug, Clone)]
pub struct ChannelView {
    session: SessionView,
    /// The document as it was when the channel was reached.
    channel: Arc<Channel>,
    /// Whether it runs; `None` if its session does not.
    run: Option<ChannelRun>,
    /// How far the viewer reads it (`channel_reading`).
    reading: Reading,
}

impl ChannelView {
    pub(super) fn of(session: SessionView, channel: Arc<Channel>, run: Option<ChannelRun>, reading: Reading) -> Self {
        Self { session, channel, run, reading }
    }

    /// The views the channel was reached through - what a plugin's
    /// extension of the channel's view reads with (issue 328).
    pub fn views(&self) -> &Views {
        self.session.views()
    }

    /// How the viewer reads the channel: as a whole, or only what was
    /// released to where they read from (issues 133, 421).
    pub fn reading(&self) -> &Reading {
        &self.reading
    }

    /// The session the channel belongs to.
    pub fn session(&self) -> &SessionView {
        &self.session
    }

    /// The workspace of the session, if the viewer may use it in this
    /// channel (issue 251, decision 10): as a current participant, the channel
    /// using it, and it and the channel flowing both ways. Nothing
    /// otherwise - also not why, nor that there is one.
    ///
    /// # Errors
    /// `Failed` if reading the workspace failed.
    pub async fn workspace(&self) -> ViewResult<Option<WorkspaceView>> {
        let session = self.session.as_session();
        let Some(id) = session.workspace else { return Ok(None) };
        let Some(workspace) = self.views().controller_manager().workspace_controller(id).await.optional()
            .map_err(|e| ViewError::failed(e, "reading the session's workspace"))? else { return Ok(None) };
        let usable = workspace.usable_in(self.views().context(), session, &self.channel).await
            .map_err(|e| ViewError::failed(e, "reading the session's workspace"))?;
        Ok(usable.map(|workspace| WorkspaceView::of(self.views().clone(), workspace)))
    }

    pub fn id(&self) -> ChannelId {
        self.channel.id
    }

    pub fn name(&self) -> &Name {
        &self.channel.name
    }

    /// The kind of the channel's moderator, e.g. `policy_moderator` (issue
    /// 174); `None` if it comes from a component bundle.
    pub fn moderator(&self) -> Option<String> {
        self.views().components().kind_of::<dyn Moderator>(&self.channel.components)
    }

    /// The kind of the policy configured for the moderator's slot, e.g.
    /// `round_robin_policy` (issues 44, 174); `None` if it comes from a
    /// component bundle.
    pub fn responder_policy(&self) -> Option<String> {
        self.views().components().kind_of::<dyn ResponderPolicy>(&self.channel.components)
    }

    /// The classification of the channel's contents (issue 133).
    pub fn classification(&self) -> &Classification {
        &self.channel.classification
    }

    /// The language of its own the channel's conversation is held in (issue
    /// 56), if any.
    pub fn language(&self) -> Option<&Language> {
        self.channel.language.as_ref()
    }

    /// The language the channel's conversation is held in (issue 425): its
    /// own, else its session's as it was when the channel was reached, if
    /// any.
    pub fn effective_language(&self) -> Option<&Language> {
        self.channel.effective_language(self.session.as_session())
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

    /// Whether `actor` is among the channel's participants - resting in it
    /// included.
    pub fn lists(&self, actor: ActorId) -> bool {
        self.channel.participants.iter().any(|p| p.actor_id() == actor)
    }

    /// Whether the channel runs: its loop is where an operation goes.
    pub fn running(&self) -> Option<&Arc<ChannelLoop>> {
        match &self.run {
            Some(ChannelRun::Running(channel_loop)) => Some(channel_loop),
            Some(ChannelRun::Unavailable(_)) | None => None,
        }
    }

    /// Why the channel does not run though its session does (issue 463), as
    /// it may be shown: its components cannot be read. `None` if it runs, or
    /// its session does not.
    fn unavailable(&self) -> Option<String> {
        match &self.run {
            Some(ChannelRun::Unavailable(cause)) => component_message(cause),
            Some(ChannelRun::Running(_)) | None => None,
        }
    }

    /// The channel as the viewer follows it, from now on (`ChannelWatch`,
    /// issues 104, 386): what their own actor in it shows, across its runs,
    /// whether the channel runs or not; it is not started for this. `None`
    /// if the viewer is no user taking part in it.
    pub fn user_follow(&self) -> Option<ChannelWatch> {
        let viewer @ ActorId::User(_) = self.views().context().caller_id else { return None };
        self.lists(viewer).then(|| ChannelWatch::of(&self.views().actor_manager().outbox(self.channel.id, viewer)))
    }

    /// Whether the viewer may read an entry of the channel classified
    /// `classification`: every one if it reads the channel as a whole, else
    /// those released to where it reads from.
    pub(super) fn may_read_entry(&self, classification: &Classification) -> bool {
        self.reading.admits(classification, self.views().confidentiality().compartments())
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
        self.views().access_policy().may_change_channel(self.views().context(), &self.channel.security_attributes).await
    }

    /// Its participants, in the order of the channel - but the deleted
    /// agents resting in it (issue 314), which are former participants.
    pub async fn participants(&self) -> Vec<ParticipantView> {
        let mut participants = Vec::with_capacity(self.channel.participants.len());
        for participant in &self.channel.participants {
            if self.takes_part(participant).await {
                participants.push(ParticipantView::of(self.clone(), participant.clone()).await);
            }
        }
        participants
    }

    /// Who took part and left (issue 164), in the order they left, then the
    /// deleted agents resting in the channel (issue 314), removed as their
    /// mark says - not even that once they were purged: their messages are
    /// in the history and an agent's state in the store, so what it saw
    /// stays readable.
    pub async fn former_participants(&self) -> Vec<ParticipantView> {
        let mut former = Vec::with_capacity(self.channel.former.len());
        for left in &self.channel.former {
            former.push(ParticipantView::of_former(self.clone(), left.participant.clone(), Some(left.left.clone())).await);
        }
        for participant in &self.channel.participants {
            if !self.takes_part(participant).await {
                former.push(self.resting(participant).await);
            }
        }
        former
    }

    /// The participant `actor` of the channel - or one that took part and left,
    /// a former participant (`ParticipantView::former`, issues 163, 164), a deleted
    /// agent resting in it among them (issue 314). An agent that left
    /// before the channel kept who left is known by the messages it left in
    /// the history the viewer sees.
    ///
    /// # Errors
    /// `ParticipantNotFound` if `actor` neither takes part in the channel nor
    /// left it; `Failed` if the history cannot be read.
    pub async fn participant(&self, actor: ActorId) -> ViewResult<ParticipantView> {
        if let Some(participant) = self.channel.participants.iter().find(|p| p.actor_id() == actor) {
            return Ok(match self.takes_part(participant).await {
                true => ParticipantView::of(self.clone(), participant.clone()).await,
                false => self.resting(participant).await,
            });
        }
        if let Some(left) = self.channel.former.iter().find(|f| f.participant.actor_id() == actor) {
            return Ok(ParticipantView::of_former(self.clone(), left.participant.clone(), Some(left.left.clone())).await);
        }
        let not_found = || ViewError::ParticipantNotFound { channel: self.channel.id, actor };
        let ActorId::Agent(agent) = actor else { return Err(not_found()) };
        if self.history_page_of(None, 1, Some(Filter::said_by(actor))).await?.items.is_empty() {
            return Err(not_found());
        }
        Ok(ParticipantView::of_former(self.clone(), Participant::Agent(agent), None).await)
    }

    /// Whether `participant` takes part in the channel now: not an agent
    /// that was deleted, which rests in it until it is restored (issue 314).
    async fn takes_part(&self, participant: &Participant) -> bool {
        match participant {
            Participant::Agent(id) => self.views().agent_of(*id).await.is_some(),
            _ => true,
        }
    }

    /// `participant`, a deleted agent resting in the channel, as a former
    /// participant: removed as its mark says, unknown when once it was purged.
    async fn resting(&self, participant: &Participant) -> ParticipantView {
        let left = match participant {
            Participant::Agent(id) => self.views().agent_removal(*id).await,
            _ => None,
        };
        ParticipantView::of_former(self.clone(), participant.clone(), left).await
    }

    /// The position from which on the viewer sees the history (issues 46,
    /// 417): where the first range of their membership begins - all of it,
    /// `0`, unless they joined later and were given less, or never were a
    /// member. An agent reads as itself, not as the owner it acts for: its
    /// own membership counts, not the owner's. What a page holds is the
    /// store's to decide by every range; this says where it begins.
    ///
    /// # Errors
    /// `Failed` if the history cannot be read.
    pub async fn visible_from(&self) -> ViewResult<Position> {
        let ranges = self.views().histories().memberships(self.channel.id, self.reader()).await.map_err(|e| self.failed_reading(e))?;
        Ok(ranges.first().map_or(Position::START, |range| range.start))
    }

    /// A page of the history as the viewer sees it (issues 46, 218, 379,
    /// 417): the last `limit` items before the position `before` - the last
    /// ones without it - of those they see; of a channel read
    /// entry by entry, the messages released to where the viewer reads
    /// from, and nothing of how turns ended. Read from the store, whether the
    /// channel runs or not.
    ///
    /// # Errors
    /// `Failed` if the history cannot be read; `NoChannelNamed` if nothing
    /// of a channel read entry by entry can be on this page.
    pub async fn history_page(&self, before: Option<Position>, limit: usize) -> ViewResult<HistoryPage> {
        self.history_page_of(before, limit, None).await
    }

    /// A page of the items of the history that meet `filter`, as
    /// `history_page` gives them: what a search of the history reads - the
    /// last messages containing words, those of one sender.
    ///
    /// # Errors
    /// As `history_page`.
    pub async fn history_page_of(&self, before: Option<Position>, limit: usize, filter: Option<Filter<HistoryField>>) -> ViewResult<HistoryPage> {
        let filter = self.readable_items(filter);
        let page = self.views().histories().page(self.channel.id, &self.reading_as(), Position::START, before, limit, filter.as_ref()).await
            .map_err(|e| self.failed_reading(e))?;
        if self.reading != Reading::Whole && page.items.is_empty() {
            return Err(self.nothing_released());
        }
        Ok(page)
    }

    /// The message `id` as it stands (issue 218), if the viewer sees it in
    /// the history.
    ///
    /// # Errors
    /// `MessageNotFound` if it does not; `Failed` if the history cannot be
    /// read; `NoChannelNamed` instead of `MessageNotFound` in a channel read
    /// entry by entry.
    pub async fn message(&self, id: MessageId) -> ViewResult<Message> {
        let said = self.views().histories().said(self.channel.id, &self.reading_as(), id).await.map_err(|e| self.failed_reading(e))?;
        let seen = said.and_then(|(_, item)| item.said().cloned());
        match seen {
            Some(message) if self.may_read_entry(&message.classification) => Ok(message),
            _ if self.reading != Reading::Whole => Err(self.nothing_released()),
            _ => Err(ViewError::MessageNotFound { channel: self.channel.id, message: id }),
        }
    }

    /// The attachment `id` of message `message` as it stands - one it carries, or one its tool
    /// results showed (`Message::carried`) -, if the viewer sees the message and may read what it
    /// carries (issue 282). What a result the viewer is not shown showed is not carried for them.
    ///
    /// # Errors
    /// As `message`; `Attachment` with `NotFound` if it carries no such attachment or one the
    /// viewer may not read - a channel read entry by entry releases each by its own label.
    pub async fn attachment(&self, message: MessageId, id: ContentId) -> ViewResult<ContentRef> {
        let message = self.message(message).await?;
        let attachment = message.carried()
            .find(|attachment| attachment.id == id && self.may_read_entry(&attachment.classification))
            .ok_or(AttachmentError::NotFound(id))?;
        Ok(attachment.clone())
    }

    /// The attachment `id` of message `message`, as `attachment` answers it, with its content and
    /// the size its storage gives it - not the reference's, so that what a response says it sends
    /// is what it sends, as `ArtifactView::content` answers.
    ///
    /// # Errors
    /// As `attachment`; `Attachment` with `NotFound` also if the storage has nothing under its
    /// key, `StorageGone`, `Content`.
    pub async fn attachment_content(&self, message: MessageId, id: ContentId) -> ViewResult<(ContentRef, u64, ByteStream)> {
        let attachment = self.attachment(message, id).await?;
        let (size, content) = self.views().attachment_manager().content_of(&attachment).await?;
        Ok((attachment, size, content))
    }

    /// Whoever reads the history: an agent reads as itself, not as the owner
    /// it acts for - its own limit applies, not the owner's.
    fn reader(&self) -> ActorId {
        self.views().viewer().context().caller_id
    }

    /// As whom the viewer reads the history (issue 417): as the participant
    /// they are or were, given what went to them and what they sent; as an
    /// outsider, given what went to everybody, if they take no part in the
    /// channel or read it from elsewhere, entry by entry.
    fn reading_as(&self) -> Reader {
        let reader = self.reader();
        let took_part = self.lists(reader) || self.channel.former.iter().any(|left| left.participant.actor_id() == reader);
        match self.reading {
            Reading::Whole if took_part => Reader::Participant(reader),
            _ => Reader::Outsider,
        }
    }

    /// That the history could not be read: `error`, logged.
    fn failed_reading(&self, error: StoreError) -> ViewError {
        warn!("Cannot read the history of channel {}: {}", self.channel.id, error);
        ViewError::failed(error, format!("reading the history of channel {}", self.channel.id))
    }

    /// The items of the history the viewer may read that meet `filter`, as
    /// one condition for the store (issue 421): every item if they read the
    /// channel as a whole; else the messages released, and nothing of how
    /// turns ended - that would tell of work in the channel whose messages
    /// are not released (issue 258).
    fn readable_items(&self, filter: Option<Filter<HistoryField>>) -> Option<Filter<HistoryField>> {
        match (Filter::readable(&self.reading, self.views().confidentiality().compartments()), filter) {
            (Some(readable), Some(filter)) => Some(Filter::all_of([readable, filter])),
            (readable, filter) => readable.or(filter),
        }
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
        for participant in self.participants().await {
            if let Some(notes) = participant.notes().await? {
                published.push(notes);
            }
        }
        if self.reading != Reading::Whole && published.iter().all(|p| p.notes.is_empty()) {
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
        for participant in self.participants().await {
            pending.extend(participant.pending_approvals().await?);
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
        takes_part && self.reading == Reading::Whole
    }

    /// The channel as the viewer sees it at this moment (issue 80), with
    /// the details `detail` asks for. A detail that cannot be read is left
    /// empty: the channel can be shown without it.
    /// The session's workspace, if the channel uses it and the viewer may
    /// read it as its access list lets them (issue 23).
    async fn readable_workspace(&self) -> Option<WorkspaceId> {
        let id = self.session.as_session().workspace.filter(|_| self.channel.uses_workspace)?;
        self.views().workspace(id).await.ok().map(|workspace| workspace.id())
    }

    pub async fn snapshot(&self, detail: Detail) -> ChannelSnapshot {
        let mut participants = Vec::new();
        let mut former_participants = Vec::new();
        if detail.contains(Detail::PARTICIPANTS) {
            for participant in self.participants().await {
                participants.push(participant.snapshot(detail).await);
            }
            for participant in self.former_participants().await {
                former_participants.push(participant.snapshot(Detail::empty()).await);
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
            effective_language: self.effective_language().cloned(),
            approval_rules: self.channel.approval_rules.clone(),
            uses_workspace: self.channel.uses_workspace,
            workspace_id: self.readable_workspace().await,
            can_change: self.may_change().await,
            unavailable: self.unavailable(),
            participants,
            former_participants,
            unread: match detail.contains(Detail::UNREAD) {
                true => self.unread().await.ok().flatten(),
                false => None,
            },
        }
    }

    /// What is new in the channel to the viewer (issue 404), if they are a
    /// user: the messages others said that the viewer hears and may read,
    /// after the last one they read and from where their view of the
    /// history begins - counted up to `Unread::CAP` -, and whether one of
    /// them mentions them. Read from the store, whether the channel runs or
    /// not.
    ///
    /// # Errors
    /// `Failed` if the history or what the user read cannot be read.
    pub async fn unread(&self) -> ViewResult<Option<Unread>> {
        let viewer @ ActorId::User(user) = self.reader() else { return Ok(None) };
        let state = self.views().states().get_user_channel_state(user, self.channel.id).await.map_err(|e| {
            warn!("Cannot read what user {} read of channel {}: {}", user, self.channel.id, e);
            ViewError::failed(e, format!("reading what was read of channel {}", self.channel.id))
        })?;
        let read_up_to = state.read.map(|read| read.message);
        let from = state.unread_from();
        let new = || Filter::all_of([Filter::heard(), Filter::negated(Filter::said_by(viewer))]);
        let (new, mentioning) = (self.readable_items(Some(new())), self.readable_items(Some(Filter::all_of([new(), Filter::mentioning(viewer)]))));
        let key = self.channel.id;
        let reader = Reader::Participant(viewer);
        let counted = self.views().histories().page(key, &reader, from, None, Unread::CAP, new.as_ref()).await.map_err(|e| self.failed_reading(e))?;
        // A mention among those not counted is looked for on its own.
        let mentions_viewer = |item: &HistoryEntry| item.said().is_some_and(|message| message.mentions.contains(&viewer));
        let mentioned = counted.entries().any(mentions_viewer) || (counted.items.len() == Unread::CAP
            && !self.views().histories().page(key, &reader, from, None, 1, mentioning.as_ref()).await.map_err(|e| self.failed_reading(e))?.items.is_empty());
        Ok(Some(Unread { messages: counted.items.len(), mentioned, read_up_to }))
    }
}

/// A participant of a channel - a user or an agent - as the
/// viewer reached it. What differs between the kinds is its `ParticipantState`;
/// what the viewer may see of it is decided here, for every kind alike.
#[derive(Debug, Clone)]
pub struct ParticipantView {
    channel: ChannelView,
    participant: Participant,
    /// Took part once and left.
    former: bool,
    /// Who removed it and when, if the channel kept it (issue 164).
    left: Option<Removal>,
    /// What the participant is, as its kind makes it.
    state: Arc<dyn ParticipantState>,
}

impl ParticipantView {
    /// `participant`, who takes part in `channel`.
    async fn of(channel: ChannelView, participant: Participant) -> Self {
        let state = participant_state(channel.views(), &participant, channel.id()).await;
        Self { channel, participant, former: false, left: None, state }
    }

    /// `participant`, who took part in `channel` and left - removed as
    /// `left` says, if the channel kept it.
    async fn of_former(channel: ChannelView, participant: Participant, left: Option<Removal>) -> Self {
        let state = participant_state(channel.views(), &participant, channel.id()).await;
        Self { channel, participant, former: true, left, state }
    }

    /// Whether the participant took part once and left: not among the channel's
    /// participants any more, but what it said and saw is still there.
    pub fn former(&self) -> bool {
        self.former
    }

    /// Who removed the participant and when (issue 164); `None` for one that
    /// takes part, and for one that left before the channel kept it.
    pub fn left(&self) -> Option<&Removal> {
        self.left.as_ref()
    }

    /// The channel the participant takes part in.
    pub fn channel(&self) -> &ChannelView {
        &self.channel
    }

    /// The views the participant was reached through: in whose name it is read.
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

    /// The handle the participant is `@`-mentioned by (see `crate::mention`), or
    /// `None` if it cannot be mentioned - participants that no longer exist,
    /// names that yield no handle.
    pub fn handle(&self) -> Option<String> {
        self.state.handle()
    }

    /// How the participant is introduced (issue 176): an agent as its
    /// personality introduces it, a user as they introduce themselves in
    /// their profile. `None` for a participant that has no introduction, and
    /// for one that no longer exists. Public, like the
    /// name.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    pub async fn introduction(&self) -> ViewResult<Option<Introduction>> {
        self.state.introduction().await
    }

    /// What of the participant is public to everyone in the channel (issue
    /// 403): its name and introduction, as `name` and `introduction` give
    /// them. `None` for a participant that no longer exists.
    ///
    /// # Errors
    /// `Failed` if its introduction cannot be read.
    pub async fn describe(&self) -> ViewResult<Option<ActorDescription>> {
        self.state.describe(self.id()).await
    }

    /// The name of an agent participant's personality, without its scope, and
    /// whether the agent runs here or elsewhere (issue 244). Public, like
    /// its introduction - not the personality itself, which stays its
    /// readers'. `None` for a participant that is no agent and for an agent
    /// whose personality is gone.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    pub async fn personality(&self) -> ViewResult<Option<(String, PersonalityKind)>> {
        Ok(self.state.personality().await?.map(|personality| (personality.name.to_string(), personality.kind)))
    }

    /// Who owns an agent participant (issue 244): whose authority it acts with
    /// and who answers for its tool calls. `None` for a participant that is no
    /// agent or no longer exists.
    pub fn owner(&self) -> Option<ActorId> {
        self.state.attributes().map(|attributes| attributes.owner_id)
    }

    /// Whether the viewer may see what this participant saw when it produced a
    /// message (`ReadAgentContext`, issue 41) - only ever for agents.
    pub async fn may_read_context(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) => self.views().access_policy().may_read_agent_context(self.views().context(), attributes).await,
            None => false,
        }
    }

    /// Whether the viewer may see which tools the participant has now
    /// (`ReadAgentTools`, issue 194) - only ever for agents.
    pub async fn may_read_tools(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) => self.views().access_policy().may_read_agent_tools(self.views().context(), attributes, &self.channel.channel.security_attributes).await,
            None => false,
        }
    }

    /// Whether the viewer may see which components the participant was built
    /// from (`ReadAgentComponents`, issue 270) - only ever for agents.
    pub async fn may_read_components(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) => self.views().access_policy().may_read_agent_components(self.views().context(), attributes).await,
            None => false,
        }
    }

    /// Whether the viewer may approve the tool calls of the participant
    /// (`ApproveToolCall`, issue 14) - only ever for agents taking part.
    /// Who may is asked when it calls a tool needing approval; everybody
    /// else sees only that it waits.
    pub async fn may_approve(&self) -> bool {
        match self.state.attributes() {
            Some(attributes) if !self.former => self.views().access_policy().may_approve(self.views().context(), attributes, &self.channel.channel.security_attributes).await,
            _ => false,
        }
    }

    /// Whether the viewer may ask the participant to answer again instead of one
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
                self.views().access_policy().may_retry_message(self.views().context(), &self.channel.channel.security_attributes).await
            }
            Ok(_) => false,
            Err(e) => {
                warn!("Reading whether {} can retry failed: {:#}", self.id(), e);
                false
            }
        }
    }

    /// Whether the viewer may end the participant's answer under way
    /// (`CancelChannel`, issues 235, 243) - only ever an agent taking part,
    /// of either kind.
    pub async fn may_cancel(&self) -> bool {
        match self.state.agent() {
            Some(_) if !self.former => self.views().access_policy().may_cancel_channel(self.views().context(), &self.channel.channel.security_attributes).await,
            _ => false,
        }
    }

    /// Where the participant stands now (issue 243) - only ever an agent taking
    /// part, read from the running channel; none in a channel that does not
    /// run.
    pub fn state(&self) -> Option<ActorState> {
        self.state.agent()?;
        self.channel.running().filter(|_| !self.former)?.actor(self.id()).map(|actor| actor.state())
    }

    /// The commands the participant carries out itself that the viewer may see
    /// (issue 269), read from its actor, which asks the policy: none if it
    /// carries out none here - it is no agent, has left - and none while the
    /// channel does not run, when nothing could carry them out.
    pub async fn commands(&self) -> Vec<Command> {
        if self.former {
            return Vec::new();
        }
        let Some(actor) = self.channel.running().and_then(|channel| channel.actor(self.id())) else { return Vec::new() };
        actor.commands(self.views().context()).await
    }

    /// The tools the participant has now, in their tool sets (issue 194), as it
    /// would offer them at the start of its next turn - read from its
    /// actor, since an agent's tools are built for each of its channels.
    /// None if it has none here: it is no agent run here or has left - and
    /// also while the channel does not run, when its tools are not built.
    ///
    /// # Errors
    /// `Forbidden` if the participant is an agent whose tools the viewer may not
    /// see (`ReadAgentTools`).
    pub async fn tool_sets(&self) -> ViewResult<Vec<ToolSet>> {
        let Some(attributes) = self.state.attributes() else { return Ok(Vec::new()) };
        let views = self.views();
        views.access_policy()
            .require(views.context(), ActionType::ReadAgentTools { agent: attributes, channel: &self.channel.channel.security_attributes }, |refused| ViewError::Forbidden { actor: self.id(), refused }).await?;
        if self.former {
            return Ok(Vec::new());
        }
        let Some(actor) = self.channel.running().and_then(|channel| channel.actor(self.id())) else { return Ok(Vec::new()) };
        Ok(actor.tool_sets().await)
    }

    /// The components the participant was built from in this channel, the bundles it includes
    /// resolved (issue 270) - read from its actor, which recorded them when it was built: what it
    /// is, not what its personality would make of it now. None if it is no agent run here or has
    /// left - and also while the channel does not run.
    ///
    /// # Errors
    /// `Forbidden` if the participant is an agent whose components the viewer
    /// may not see (`ReadAgentComponents`).
    pub async fn components(&self) -> ViewResult<ComponentConfig> {
        let Some(attributes) = self.state.attributes() else { return Ok(ComponentConfig::default()) };
        let views = self.views();
        views.access_policy()
            .require(views.context(), ActionType::ReadAgentComponents(attributes), |refused| ViewError::Forbidden { actor: self.id(), refused }).await?;
        if self.former {
            return Ok(ComponentConfig::default());
        }
        let Some(actor) = self.channel.running().and_then(|channel| channel.actor(self.id())) else { return Ok(ComponentConfig::default()) };
        Ok(actor.components())
    }

    /// Whether the viewer may delete the participant - an agent that exists,
    /// its owner (`DeleteAgent`, issue 164).
    pub async fn may_delete(&self) -> bool {
        match (self.state.agent(), self.state.attributes()) {
            (Some(_), Some(attributes)) => self.views().access_policy().may_delete_agent(self.views().context(), attributes).await,
            _ => false,
        }
    }

    /// The notes the participant published in the channel (issue 17), with the
    /// digest of their chain - of a channel read entry by entry, those
    /// released to where the viewer reads from; `None` for a participant that
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
        self.channel.released(state.notes.current())
    }

    /// The approvals the participant's turn waits for (issue 172), as stored;
    /// none for a participant that is no agent or has left.
    ///
    /// # Errors
    /// `Failed` if the agent's state cannot be read.
    async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>> {
        match self.state.agent() {
            Some(agent) if !self.former => Ok(agent.stored().await?.pending_approvals()),
            _ => Ok(Vec::new()),
        }
    }

    /// The private notes the participant wrote in the channel (issue 51), with
    /// the digest of their chain, for a viewer who may read what it saw
    /// (`ReadAgentContext`) - of a channel read entry by entry, those
    /// released to where the viewer reads from (issue 133).
    ///
    /// # Errors
    /// `NoContext` if the participant is no agent, `Forbidden` if the viewer may
    /// not read what it saw, `Failed` if its state cannot be read.
    pub async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        let agent = self.readable_agent().await?;
        let (notes, digest) = agent.private_notes().await?;
        Ok((self.channel.released(notes), digest))
    }

    /// What the participant saw in the channel (issue 41): its transcript, its
    /// private notes, the context of each of its messages, the turns it
    /// gave up - of an external agent, what the remote agent was given
    /// (issue 179) - for a viewer who may read its context
    /// (`ReadAgentContext`) and the channel as a whole.
    ///
    /// # Errors
    /// `NoContext` if the participant is no agent, `Forbidden` if the viewer may
    /// not read what it saw or reads the channel entry by entry only,
    /// `Failed` if its state cannot be read.
    pub async fn context(&self) -> ViewResult<AgentContextView> {
        let agent = self.readable_agent().await?;
        if self.channel.reading != Reading::Whole {
            let attributes = self.state.attributes().ok_or(ViewError::NoContext { actor: self.id() })?;
            return Err(ViewError::Forbidden { actor: self.id(), refused: ActionType::ReadAgentContext(attributes).refused() });
        }
        agent.context().await
    }

    /// The participant as an agent whose context the viewer may read.
    async fn readable_agent(&self) -> ViewResult<&AgentParticipant> {
        let no_context = || ViewError::NoContext { actor: self.id() };
        let attributes = self.state.attributes().ok_or_else(no_context)?;
        let views = self.views();
        views.access_policy()
            .require(views.context(), ActionType::ReadAgentContext(attributes), |refused| ViewError::Forbidden { actor: self.id(), refused }).await?;
        self.state.agent().ok_or_else(no_context)
    }

    /// The turn of the answer the participant is giving now (issue 240), from the
    /// running channel - none of a former participant, nor in a channel that does
    /// not run.
    fn turn_under_way(&self) -> Option<TurnId> {
        self.channel.running().filter(|_| !self.former)?.turn_under_way(self.id())
    }

    /// The participant as the viewer sees it at this moment (issue 80), with
    /// the details `detail` asks for - those of an agent taken from its
    /// state, read once for all of them; who it is (`PROFILE`) from its
    /// personality, its owner and a user's profile. What cannot be read is
    /// left out.
    pub async fn snapshot(&self, detail: Detail) -> ChannelParticipantSnapshot {
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
                .ok().flatten().map(String::from);
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
                usage = state.used;
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
        ChannelParticipantSnapshot {
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
    fn released(&self, notes: Vec<Note>) -> Vec<Note> {
        if self.reading == Reading::Whole {
            return notes;
        }
        notes.into_iter().filter(|note| self.may_read_entry(&note.classification)).collect()
    }
}
