//! What an application reads of its session (issue 516): the readers, each the part of a view an
//! agent's work reads through, and the extensions a plugin of the core binds to the views of a
//! piece of work. The views implement the readers and keep their full surface for the frontends;
//! an application is handed a reader, never a view, so it reaches no policy, no manager and no
//! context through it.

use super::WorkspaceView;
use super::{ChannelView, ParticipantView, PrivateNotesElsewhere, SessionView, ViewResult, Views};
use crate::terminal::ByteStream;
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId};
use caelumora_model::chain::Position;
use caelumora_model::confidentiality::{Classification, Place};
use caelumora_model::content::{ContentId, ContentRef};
use caelumora_model::history::HistoryPage;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::project::ProjectId;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::PublishedNotes;
use caelumora_model::store::history::HistoryField;
use caelumora_model::store::query::Filter;
use caelumora_model::type_map::{ErasedArc, TypeKey};
use caelumora_model::value::language::Language;
use caelumora_model::value::name::Name;
use caelumora_model::value::text::{Briefing, Description, Summary};
use caelumora_model::workspace::WorkspaceId;
use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A session as an agent's work reads it from its channel (`SessionView`).
#[async_trait]
pub trait SessionReader: Send + Sync {
    /// The session read.
    fn id(&self) -> SessionId;

    /// The project its results go to (issue 467).
    fn project(&self) -> ProjectId;

    /// What the session is for, in a line (issue 245).
    fn summary(&self) -> Option<&Summary>;

    /// What an agent should know before it works in the session (issue 245).
    fn briefing(&self) -> Option<&Briefing>;

    /// Where the reading stands: the actor's channel, at the classification its work reads at.
    fn place(&self) -> Option<&Place>;

    /// Channel `id` of the session, if the reader may read it.
    async fn channel(&self, id: ChannelId) -> ViewResult<Box<dyn ChannelReader>>;

    /// The channel of the session called `name`, if the reader may read it.
    async fn channel_named(&self, name: &str) -> ViewResult<Box<dyn ChannelReader>>;

    /// The channels of the session the reader may read.
    async fn channels(&self) -> Vec<Box<dyn ChannelReader>>;

    /// The reader's private notes in the session's other channels (issue 255).
    async fn own_private_notes(&self) -> Vec<PrivateNotesElsewhere>;

    /// How session `id` stands for the flow rule now, `None` if it is gone (issue 134).
    async fn privacy_of(&self, id: SessionId) -> ViewResult<Option<SessionPrivacy>>;
}

/// A channel as an agent's work reads it (`ChannelView`): as a whole, or what of it it may read.
#[async_trait]
pub trait ChannelReader: Send + Sync {
    /// The channel read.
    fn id(&self) -> ChannelId;

    /// Its name, unique in its session.
    fn name(&self) -> &Name;

    /// How its contents are classified (issue 133).
    fn classification(&self) -> &Classification;

    /// What the channel is for, in a line (issue 245).
    fn summary(&self) -> Option<&Summary>;

    /// What an agent should know before it works in the channel (issue 245).
    fn briefing(&self) -> Option<&Briefing>;

    /// The language the channel's conversation is held in: its own, else its session's (issue
    /// 425).
    fn effective_language(&self) -> Option<&Language>;

    /// The session it was reached through.
    fn session(&self) -> &dyn SessionReader;

    /// The workspace of the session, if the reader may use it in this channel (issue 290).
    async fn workspace(&self) -> ViewResult<Option<Box<dyn WorkspaceReader>>>;

    /// Its participants, each as the reader may see them.
    async fn participants(&self) -> Vec<Box<dyn ParticipantReader>>;

    /// Its participant `actor`, if it takes part.
    async fn participant(&self, actor: ActorId) -> ViewResult<Box<dyn ParticipantReader>>;

    /// Where in the history the reader's reading begins (issue 379).
    async fn visible_from(&self) -> ViewResult<Position>;

    /// A page of the history the reader may read, before `before`, of at most `limit` entries
    /// that match `filter`.
    async fn history_page_of(
        &self,
        before: Option<Position>,
        limit: usize,
        filter: Option<Filter<HistoryField>>,
    ) -> ViewResult<HistoryPage>;

    /// Message `id`, if the reader may read it.
    async fn message(&self, id: MessageId) -> ViewResult<Message>;

    /// What message `message` carries as `id`, if the reader may read it.
    async fn attachment(&self, message: MessageId, id: ContentId) -> ViewResult<ContentRef>;

    /// What message `message` carries as `id`, with its size and its content.
    async fn attachment_content(
        &self,
        message: MessageId,
        id: ContentId,
    ) -> ViewResult<(ContentRef, u64, ByteStream)>;

    /// The notes its participants published there, as far as the reader may read them.
    async fn notes(&self) -> ViewResult<Vec<PublishedNotes>>;
}

/// A participant of a channel as an agent's work sees it (`ParticipantView`).
#[async_trait]
pub trait ParticipantReader: Send + Sync {
    /// The actor that takes part.
    fn id(&self) -> ActorId;

    /// Its name; `None` for a user or agent that no longer exists.
    fn name(&self) -> Option<&str>;

    /// What of the participant is public to everyone in the channel (issue 176); `None` for one
    /// that no longer exists.
    async fn describe(&self) -> ViewResult<Option<ActorDescription>>;
}

/// The workspace of a session as an agent's work sees it in its channel (`WorkspaceView`).
#[async_trait]
pub trait WorkspaceReader: Send + Sync {
    /// The workspace read.
    fn id(&self) -> WorkspaceId;

    /// Its name, unique where it lies.
    fn name(&self) -> &str;

    /// What it holds, in a line; `None` if nobody said.
    fn description(&self) -> Option<&Description>;

    /// How its files are classified.
    fn classification(&self) -> &Classification;

    /// Whether commands can be run in it (issue 344).
    fn has_shell(&self) -> bool;

    /// Whether the reader may change its files.
    async fn may_write(&self) -> bool;
}

/// What the plugins of the core bind to the views of an application's work (issue 516), each by
/// the type of what it binds: a service of the plugin's, made from the views, and so acting in
/// their viewer's name. An application asks its runtime for it (`ActorRuntime::extension_of`); it
/// names the service's trait, never the plugin's manager or the views.
#[derive(Default)]
pub(crate) struct ReaderExtensions {
    binders: HashMap<TypeId, Binder>,
}

type Binder = Arc<dyn Fn(&Views) -> ErasedArc + Send + Sync>;

impl fmt::Debug for ReaderExtensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReaderExtensions").field("binders", &self.binders.len()).finish()
    }
}

impl ReaderExtensions {
    /// Binds `T` to the views of every piece of work by `bind`. A `T` bound before is replaced.
    pub(crate) fn insert<T, F>(&mut self, bind: F)
    where
        T: ?Sized + Send + Sync + 'static,
        F: Fn(&Views) -> Arc<T> + Send + Sync + 'static,
    {
        let binder: Binder = Arc::new(move |views| ErasedArc::of(bind(views)));
        self.binders.insert(TypeKey::of::<T>().id, binder);
    }

    /// What is bound as the type `key` names, made from `views`, if anything is.
    pub(crate) fn bind(&self, key: TypeKey, views: &Views) -> Option<ErasedArc> {
        self.binders.get(&key.id).map(|bind| bind(views))
    }
}

#[async_trait]
impl SessionReader for SessionView {
    fn id(&self) -> SessionId {
        SessionView::id(self)
    }

    fn project(&self) -> ProjectId {
        SessionView::project(self)
    }

    fn summary(&self) -> Option<&Summary> {
        SessionView::summary(self)
    }

    fn briefing(&self) -> Option<&Briefing> {
        SessionView::briefing(self)
    }

    fn place(&self) -> Option<&Place> {
        self.views().viewer().place()
    }

    async fn channel(&self, id: ChannelId) -> ViewResult<Box<dyn ChannelReader>> {
        Ok(Box::new(SessionView::channel(self, id).await?))
    }

    async fn channel_named(&self, name: &str) -> ViewResult<Box<dyn ChannelReader>> {
        Ok(Box::new(SessionView::channel_named(self, name).await?))
    }

    async fn channels(&self) -> Vec<Box<dyn ChannelReader>> {
        let channels = SessionView::channels(self).await;
        channels.into_iter().map(|channel| Box::new(channel) as Box<dyn ChannelReader>).collect()
    }

    async fn own_private_notes(&self) -> Vec<PrivateNotesElsewhere> {
        SessionView::own_private_notes(self).await
    }

    async fn privacy_of(&self, id: SessionId) -> ViewResult<Option<SessionPrivacy>> {
        self.views().privacy_of(id).await
    }
}

#[async_trait]
impl ChannelReader for ChannelView {
    fn id(&self) -> ChannelId {
        ChannelView::id(self)
    }

    fn name(&self) -> &Name {
        ChannelView::name(self)
    }

    fn classification(&self) -> &Classification {
        ChannelView::classification(self)
    }

    fn summary(&self) -> Option<&Summary> {
        ChannelView::summary(self)
    }

    fn briefing(&self) -> Option<&Briefing> {
        ChannelView::briefing(self)
    }

    fn effective_language(&self) -> Option<&Language> {
        ChannelView::effective_language(self)
    }

    fn session(&self) -> &dyn SessionReader {
        ChannelView::session(self)
    }

    async fn workspace(&self) -> ViewResult<Option<Box<dyn WorkspaceReader>>> {
        let workspace = ChannelView::workspace(self).await?;
        Ok(workspace.map(|workspace| Box::new(workspace) as Box<dyn WorkspaceReader>))
    }

    async fn participants(&self) -> Vec<Box<dyn ParticipantReader>> {
        let participants = ChannelView::participants(self).await;
        participants
            .into_iter()
            .map(|participant| Box::new(participant) as Box<dyn ParticipantReader>)
            .collect()
    }

    async fn participant(&self, actor: ActorId) -> ViewResult<Box<dyn ParticipantReader>> {
        Ok(Box::new(ChannelView::participant(self, actor).await?))
    }

    async fn visible_from(&self) -> ViewResult<Position> {
        ChannelView::visible_from(self).await
    }

    async fn history_page_of(
        &self,
        before: Option<Position>,
        limit: usize,
        filter: Option<Filter<HistoryField>>,
    ) -> ViewResult<HistoryPage> {
        ChannelView::history_page_of(self, before, limit, filter).await
    }

    async fn message(&self, id: MessageId) -> ViewResult<Message> {
        ChannelView::message(self, id).await
    }

    async fn attachment(&self, message: MessageId, id: ContentId) -> ViewResult<ContentRef> {
        ChannelView::attachment(self, message, id).await
    }

    async fn attachment_content(
        &self,
        message: MessageId,
        id: ContentId,
    ) -> ViewResult<(ContentRef, u64, ByteStream)> {
        ChannelView::attachment_content(self, message, id).await
    }

    async fn notes(&self) -> ViewResult<Vec<PublishedNotes>> {
        ChannelView::notes(self).await
    }
}

#[async_trait]
impl ParticipantReader for ParticipantView {
    fn id(&self) -> ActorId {
        ParticipantView::id(self)
    }

    fn name(&self) -> Option<&str> {
        ParticipantView::name(self)
    }

    async fn describe(&self) -> ViewResult<Option<ActorDescription>> {
        ParticipantView::describe(self).await
    }
}

#[async_trait]
impl WorkspaceReader for WorkspaceView {
    fn id(&self) -> WorkspaceId {
        WorkspaceView::id(self)
    }

    fn name(&self) -> &str {
        WorkspaceView::name(self)
    }

    fn description(&self) -> Option<&Description> {
        WorkspaceView::description(self)
    }

    fn classification(&self) -> &Classification {
        WorkspaceView::classification(self)
    }

    fn has_shell(&self) -> bool {
        WorkspaceView::has_shell(self)
    }

    async fn may_write(&self) -> bool {
        WorkspaceView::may_write(self).await
    }
}
