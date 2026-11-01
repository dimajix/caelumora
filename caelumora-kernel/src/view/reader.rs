//! How the views serve the readers of the API (issue 516): each reader is the part of a view an
//! agent's work reads through (`caelumora_api::reader`).

use super::{ChannelView, ParticipantView, SessionView};
use async_trait::async_trait;
use caelumora_api::reader::{ChannelReader, ParticipantReader, SessionReader};
use caelumora_api::reader::{PrivateNotesElsewhere, ViewResult};
use caelumora_api::terminal::ByteStream;
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
use caelumora_model::value::language::Language;
use caelumora_model::value::name::Name;
use caelumora_model::value::text::{Briefing, Summary};

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
