//! The `channel_language` component (issue 56): tells the
//! agent which language its channel's conversation is held in, so that it
//! writes in it - although its instructions, its tools and the parts the
//! agents add are in English, which pulls a model towards English.
//! Nothing is translated: the conversation is one, in one language. The
//! language is named in English and in itself, and by its tag.
//!
//! The language is the channel's own, else its session's (issue 425,
//! `Channel::effective_language`), read afresh at the start of every turn
//! through the agent's views, so that a change of either reaches it with its
//! next turn. It goes into the system prompt by default (parameter
//! `position`): it changes rarely. A channel held in no language in
//! particular adds nothing. A personality without the component leaves its
//! agents to choose themselves.
//!
//! An agent that keeps what it was told - an external one (issue 204) - is
//! told that the channel is no longer held in a language once it is not
//! (`PromptProvider::withdrawn`).

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::value::language::Language;
use std::sync::Arc;

pub const CHANNEL_LANGUAGE_KIND: &str = "channel_language";

/// Its texts in the prompt catalogue: the language, and that the channel
/// is no longer held in one.
const TEXT: &str = "agents.channel_language";
const WITHDRAWN: &str = "agents.channel_language_withdrawn";

#[derive(Debug)]
pub struct ChannelLanguageComponent {
    pub position: PromptPosition,
    prompts: PromptTexts,
}

impl ChannelLanguageComponent {
    /// The language at `position`.
    pub fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, prompts }
    }
}

#[async_trait]
impl PromptProvider for ChannelLanguageComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // Work that reaches no session (`Situation::channel`) has no language to follow.
        let Some(channel) = situation.channel().await? else {
            return Ok(Prompt::classified(String::new(), Classification::PUBLIC));
        };
        let text = render(&self.prompts, channel.effective_language())?;
        // Which language a channel is held in is no secret of its contents.
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }

    fn withdrawn(&self) -> Result<Option<String>, ContextError> {
        Ok(Some(self.prompts.render(WITHDRAWN, ())?))
    }
}

/// The prompt part of `prompts` telling the agent to write in `language`;
/// empty if the channel is held in none in particular. The language is
/// named in English and in itself where `Language::names` knows it - a
/// model follows a name more surely than a tag - and always by its tag,
/// which says the region.
fn render(prompts: &PromptTexts, language: Option<&Language>) -> Result<String, PromptError> {
    let Some(language) = language else { return Ok(String::new()) };
    let (english, native) = language.names().unzip();
    prompts.render(TEXT, context! { tag => language.to_string(), english, native })
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CHANNEL_LANGUAGE_KIND)
                .params::<PositionParams>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let position = PositionParams::position_of(&descriptor.params, PromptPosition::Start)?;
            let provider = ChannelLanguageComponent::new(position, prompts);
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
    use crate::testing::{self, agent_runtime};
    use crate::turn::Turn;
    use caelumora_domain::actor::{ActorId, EventContext};
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::{Channel, Participant};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::Name;
    use caelumora_domain::work::Work;
    use caelumora_kernel::session::testing::participant_runtime_in;
    use caelumora_kernel::session::{SessionLoop, SessionManager};
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;

    fn language(tag: &str) -> Option<Language> {
        Some(Language::new(tag).unwrap())
    }

    #[test]
    fn the_agent_is_told_the_language_by_its_names_and_its_tag() {
        let prompts = testing::prompts();
        assert_eq!(render(&prompts, language("de-CH").as_ref()).unwrap(), "[Language of this channel]\n\
            This channel's conversation is held in German - Deutsch, BCP 47 tag \"de-CH\". \
            Write everything in German - your answers, your messages to the others and what you note down - \
            whatever language your instructions, your tools or earlier messages are in.\n");
        assert!(
            render(&prompts, language("en").as_ref())
                .unwrap()
                .contains("held in English (BCP 47 tag \"en\"). Write everything in English"),
            "a name once if it is the same"
        );
        let unknown = render(&prompts, language("tlh").as_ref()).unwrap();
        assert!(
            unknown.contains(
                "held in the language with the BCP 47 tag \"tlh\". Write everything in it"
            ),
            "{unknown}"
        );
        assert_eq!(
            render(&prompts, None).unwrap(),
            "",
            "a channel held in no language adds nothing"
        );
    }

    #[test]
    fn a_language_is_withdrawn_as_no_longer_held() {
        let provider = ChannelLanguageComponent::new(PromptPosition::Start, testing::prompts());
        let withdrawn = provider.withdrawn().unwrap();
        assert!(
            withdrawn
                .as_deref()
                .is_some_and(|text| text.contains("no longer held in a particular language")),
            "the language told before is taken back: {withdrawn:?}"
        );
    }

    #[tokio::test]
    async fn the_language_is_read_from_the_channel_and_its_session_at_every_turn() {
        let kernel = TestKernel { libraries: vec![&AgentsLibrary], ..Default::default() }.build();
        let system = SecurityContext::system();

        let agent = stored_agent(
            kernel.entity_stores(),
            "Jonathan",
            make_personality("plain"),
            ActorId::User(UserId::new()),
        )
        .await;
        let owner = make_user("owner");
        record_users(kernel.entity_stores(), &[&owner]).await;
        let as_owner =
            SecurityContext::new(ActorId::User(owner.id), Default::default(), Default::default());
        let session = make_session("s", owner.id);
        let mut channel = Channel::build(
            session.id,
            ActorId::User(owner.id),
            Name::new("main").unwrap(),
            Default::default(),
            Classification::CHANNEL_DEFAULT,
            language("de"),
        );
        channel.participants = vec![Participant::Agent(agent.id)];
        let session = Session {
            participants: channel.participants.clone(),
            channels: vec![channel.id],
            ..session
        };
        let session_loop = SessionManager::new(Arc::clone(&kernel))
            .create(&as_owner, session.clone(), vec![channel.clone()])
            .await
            .unwrap();
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            EventContext::new(session.id, channel.id, ActorId::None, String::new()),
            None,
            channel.classification.clone(),
            Default::default(),
        ));
        let provider = ChannelLanguageComponent::new(PromptPosition::Start, testing::prompts());
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(prompt.text.contains("\"de\""), "{}", prompt.text);
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));

        let running = session_loop.channel(channel.id).expect("the channel runs");
        running.update_language(&system, Some("fr")).await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(prompt.text.contains("\"fr\""), "the language as it is now: {}", prompt.text);

        session_loop.update_language(&system, Some("it")).await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(prompt.text.contains("\"fr\""), "its own before the session's: {}", prompt.text);
        running.update_language(&system, None).await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(
            prompt.text.contains("\"it\""),
            "without one of its own, the session's as it is now (issue 425): {}",
            prompt.text
        );
    }

    /// What `provider` tells agent `agent` in the turn begun in `state`.
    async fn told(
        provider: &ChannelLanguageComponent,
        agent: AgentId,
        session_loop: &Arc<SessionLoop>,
        state: &mut AgentChannelState,
    ) -> Prompt {
        let turn = Turn::of(
            state,
            agent_runtime(participant_runtime_in(
                &session_loop.channels()[0],
                ActorId::Agent(agent),
            )),
        )
        .expect("a turn was begun");
        provider.get_prompt(&turn.situation()).await.unwrap()
    }
}
