//! The `store_places` component (issue 577): tells the agent where the program keeps its data on
//! its host - everything, its session's and its channel's -, so that an agent asked how something
//! was kept can look there with a terminal that reaches it.
//!
//! What it tells it reads from the kernel's `StorePlacesReader`, bound where the backend of the
//! stores tells where it keeps what lies in a scope - the home store does -; elsewhere the part is
//! empty. Below the data directory lies every
//! session, user and channel of the program, so the part comes only where a personality wires the
//! kind, and says so. It never changes: in the system prompt by default (parameter `position`). It
//! names the channel's directory, and is classified as the channel.

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_api::places::StorePlacesReader;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use std::sync::Arc;

pub const STORE_PLACES_KIND: &str = "store_places";

/// Its text in the prompt catalogue.
const TEXT: &str = "agents.store_places";

#[derive(Debug)]
pub struct StorePlacesComponent {
    position: PromptPosition,
    prompts: PromptTexts,
}

impl StorePlacesComponent {
    /// Where the program keeps its data, at `position`.
    pub(crate) fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, prompts }
    }
}

#[async_trait]
impl PromptProvider for StorePlacesComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let Some(places) = situation.reader::<dyn StorePlacesReader>().await else {
            return Ok(Prompt::classified(String::new(), Classification::PUBLIC));
        };
        // It names the channel's directory: classified as the channel.
        Ok(render(&self.prompts, places.as_ref())?.into())
    }
}

/// The part of `prompts` telling where `places` lie.
fn render(prompts: &PromptTexts, places: &dyn StorePlacesReader) -> Result<String, PromptError> {
    prompts.render(
        TEXT,
        context! {
            data_dir => places.data_dir().display().to_string(),
            session_dir => places.session_dir().display().to_string(),
            channel_dir => places.channel_dir().display().to_string(),
        },
    )
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(STORE_PLACES_KIND)
                .params::<PositionParams>()
                .provides::<dyn PromptProvider>()
                .reads::<dyn StorePlacesReader>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let position = PositionParams::position_of(&descriptor.params, PromptPosition::Start)?;
            let provider = StorePlacesComponent::new(position, prompts);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(provider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::AgentsLibrary;
    use crate::testing;
    use crate::turn::Turn;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::testing::make_channel;
    use caelumora_domain::channel::Participant;
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::user::testing::make_user;
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::{record_users, stored_agent};

    /// Where the stores keep their data in files on the host - those of the world, as a store over
    /// files would -, the part names the data directory and those of the session and the channel of
    /// the work, says that more lies there than the work's, and is classified as the channel.
    #[tokio::test]
    async fn the_directories_are_named_where_the_stores_keep_files() {
        let world = testing::world().await;
        let personality = world.personality("plain", None, &[]).await;
        let jonathan = world.agent("Jonathan", world.alice, &personality).await;
        let session = world.shared_session(&[jonathan], &[]).await;
        let channel = session.channels()[0].id();
        let provider = StorePlacesComponent::new(PromptPosition::Start, testing::prompts());

        let mut state = AgentChannelState::default();
        let turn = testing::turn(&session, jonathan, world.alice, &mut state);
        let prompt = provider.get_prompt(&turn.situation()).await.unwrap();
        let session_dir = format!("/data/sessions/{}", session.id());
        assert!(
            prompt.text.contains("below the directory /data:")
                && prompt.text.contains("every session, user and channel")
                && prompt.text.contains(&format!("this session lies in {session_dir},"))
                && prompt.text.contains(&format!("{session_dir}/channels/{channel}.")),
            "the directories: {}",
            prompt.text
        );
        assert_eq!(prompt.classification, None, "as the channel");
    }

    /// No directory is told to a work that reaches no session, nor to one that may not read the
    /// channel it would be told of: the part is empty.
    #[tokio::test]
    async fn nothing_is_told_where_the_work_reads_no_channel() {
        let provider = StorePlacesComponent::new(PromptPosition::Start, testing::prompts());
        let mut state = AgentChannelState::default();
        let turn = Turn::begun(AgentId::new(), &mut state);
        let prompt = provider.get_prompt(&turn.situation()).await.unwrap();
        assert_eq!(prompt.text, "", "no session reached");

        let world = testing::world().await;
        let personality = world.personality("plain", None, &[]).await;
        let jonathan = world.agent("Jonathan", world.alice, &personality).await;
        let sassy = world.agent("Sassy", world.alice, &personality).await;
        let session = world.shared_session(&[jonathan], &[]).await;
        let mut state = AgentChannelState::default();
        let turn = testing::turn(&session, sassy, world.alice, &mut state);
        let prompt = provider.get_prompt(&turn.situation()).await.unwrap();
        assert_eq!(prompt.text, "", "an agent that takes no part in the channel");
    }

    /// Where the backend of the stores tells no places - the stores in memory -, the part is empty,
    /// in a channel the work reads.
    #[tokio::test]
    async fn nothing_is_told_where_the_stores_tell_no_places() {
        let kernel = TestKernel { libraries: vec![&AgentsLibrary], ..Default::default() }.build();
        let owner = make_user("owner");
        record_users(kernel.entity_stores(), &[&owner]).await;
        let owned_by = ActorId::User(owner.id);
        let agent =
            stored_agent(kernel.entity_stores(), "Jonathan", make_personality("plain"), owned_by)
                .await;
        let session = make_session("s", owner.id);
        let mut channel = make_channel(session.id, "main");
        channel.participants = vec![Participant::Agent(agent.id)];
        let session = Session {
            participants: channel.participants.clone(),
            channels: vec![channel.id],
            ..session
        };
        let as_owner = SecurityContext::new(owned_by, Default::default(), Default::default());
        let session_loop = kernel.create_session(&as_owner, session, vec![channel]).await.unwrap();

        let provider = StorePlacesComponent::new(PromptPosition::Start, testing::prompts());
        let mut state = AgentChannelState::default();
        let turn = testing::turn(&session_loop, agent.id, owner.id, &mut state);
        assert!(turn.situation().channel().await.unwrap().is_some(), "it reads its channel");
        let prompt = provider.get_prompt(&turn.situation()).await.unwrap();
        assert_eq!(prompt.text, "");
    }
}
