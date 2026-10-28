//! The `channel_language_prompt_provider` component (issue 56): tells the
//! agent which language its channel's conversation is held in, so that it
//! writes in it - although its instructions, its tools and the parts the
//! runtime adds are in English, which pulls a model towards English.
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
//! An agent that keeps its conversation itself - an external one (issue
//! 204) - is told the language once and then only when it changed
//! (parameter `changes_only`), as the roster is
//! (`channel_participants_prompt_provider`).

use caelumora_runtime::component::{flag_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::prompt::{ContextError, Prompt, PromptPosition, PromptProvider, texts_of, writing_for_the_model};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_model::value::language::Language;
use std::sync::{Arc, Mutex};

pub const CHANNEL_LANGUAGE_PROMPT_PROVIDER_KIND: &str = "channel_language_prompt_provider";

/// Its texts in the prompt catalogue: the language, and that the channel
/// is no longer held in one.
const TEXT: &str = "agents.channel_language";
const WITHDRAWN: &str = "agents.channel_language_withdrawn";

#[derive(Debug)]
pub struct ChannelLanguagePromptProvider {
    pub position: PromptPosition,
    /// Whether it tells only of changes: then the language it told last,
    /// `None` before it told any. A plain lock, never held across an await.
    told: Option<Mutex<Option<Option<Language>>>>,
    prompts: PromptTexts,
}

impl ChannelLanguagePromptProvider {
    /// The language at `position`, told every time it is asked.
    pub fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, told: None, prompts }
    }

    /// The language at `position`, told the first time it is asked and then
    /// only when it changed.
    pub fn changes_only(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, told: Some(Mutex::new(None)), prompts }
    }

    /// What the agent is to be told of `language` now.
    fn text(&self, language: Option<Language>) -> Result<String, PromptError> {
        let Some(told) = &self.told else { return render(&self.prompts, language.as_ref()) };
        let mut told = told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match told.as_ref() {
            Some(before) if *before == language => Ok(String::new()),
            // Held in none from the start: there is nothing to take back.
            None if language.is_none() => {
                *told = Some(None);
                Ok(String::new())
            }
            Some(Some(_)) if language.is_none() => {
                *told = Some(None);
                self.prompts.render(WITHDRAWN, ())
            }
            _ => {
                let text = render(&self.prompts, language.as_ref())?;
                *told = Some(language);
                Ok(text)
            }
        }
    }
}

#[async_trait]
impl PromptProvider for ChannelLanguagePromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // Work that reaches no session (`Situation::channel`) has no language to follow.
        let Some(channel) = situation.channel().await? else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let text = self.text(channel.effective_language().cloned())?;
        // Which language a channel is held in is no secret of its contents.
        Ok(Prompt::classified(text, Classification::PUBLIC))
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

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(CHANNEL_LANGUAGE_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>()),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let position = PromptPosition::of_params(&descriptor.params)?;
            let changes_only = flag_of_params(&descriptor.params, "changes_only")?;
            let provider = if changes_only { ChannelLanguagePromptProvider::changes_only(position, prompts) } else { ChannelLanguagePromptProvider::new(position, prompts) };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(provider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::project::ProjectId;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use crate::turn::Turn;
    use crate::testing::{self, TestAgents};
    use caelumora_runtime::testing::TestRuntime;
    use caelumora_runtime::session::{SessionLoop, SessionManager};
    use caelumora_runtime::session::testing::{actor_runtime_in, user};
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::session::{Session, SessionId};
    use caelumora_model::store::Persistence;
    use chrono::Utc;
    use caelumora_model::work::Work;

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
        assert!(render(&prompts, language("en").as_ref()).unwrap().contains("held in English (BCP 47 tag \"en\"). Write everything in English"), "a name once if it is the same");
        let unknown = render(&prompts, language("tlh").as_ref()).unwrap();
        assert!(unknown.contains("held in the language with the BCP 47 tag \"tlh\". Write everything in it"), "{unknown}");
        assert_eq!(render(&prompts, None).unwrap(), "", "a channel held in no language adds nothing");
    }

    #[test]
    fn told_every_time_unless_only_changes_are_told() {
        let always = ChannelLanguagePromptProvider::new(PromptPosition::Start, testing::prompts());
        assert_eq!(always.text(language("de")).unwrap(), always.text(language("de")).unwrap(), "every turn");

        let changes = ChannelLanguagePromptProvider::changes_only(PromptPosition::Start, testing::prompts());
        assert_eq!(changes.text(None).unwrap(), "", "held in none from the start: nothing to tell");
        assert!(changes.text(language("de")).unwrap().contains("\"de\""), "the first language");
        assert_eq!(changes.text(language("de")).unwrap(), "", "not again while it stays");
        assert!(changes.text(language("fr")).unwrap().contains("\"fr\""), "but when it changes");
        assert!(changes.text(None).unwrap().contains("no longer held in a particular language"), "and when it is taken back");
        assert_eq!(changes.text(None).unwrap(), "");
    }

    #[tokio::test]
    async fn the_language_is_read_from_the_channel_and_its_session_at_every_turn() {
        let store = Persistence::in_memory();
        let runtime = TestRuntime { store: store.clone(), plugins: vec![&TestAgents], ..Default::default() }.build();
        let system = SecurityContext::system();

        let personality = Personality {
            description: None, id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Visibility::Channel, hears_during_turn: Default::default(), deleted: None,
        };
        store.entity_stores().unwrap().personalities.create(&system, &personality).await.unwrap();
        let agent = Agent::new(ScopedName::new("Jonathan", Scope::GlobalScope).unwrap(), personality.id, ActorId::None);
        store.entity_stores().unwrap().agents.create(&system, &agent).await.unwrap();
        let owner = user("owner");
        store.entity_stores().unwrap().users.create(&system, &owner).await.unwrap();
        let as_owner = SecurityContext::new(ActorId::User(owner.id), Default::default(), Default::default());
        let session_id = SessionId::new();
        let mut channel = Channel::build(session_id, ActorId::User(owner.id), Name::new("main").unwrap(), Default::default(), Classification::CHANNEL_DEFAULT, language("de"));
        channel.participants = vec![Participant::Agent(agent.id)];
        let now = Utc::now();
        let session = Session {
            summary: None, briefing: None, language: None, time_zone: None,
            approval_rules: Vec::new(),
            workspace: None,
            // Replaced by its own, which `create` makes with it.
            project: ProjectId::new(),
            id: session_id, name: Name::new("s").unwrap(), participants: channel.participants.clone(), channels: vec![channel.id],
            commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now,
            security_attributes: Session::attributes_of(session_id, owner.id, &[]),
            deleted: None,
        };
        let session_loop = SessionManager::new(Arc::clone(&runtime)).create(&as_owner, session.clone(), vec![channel.clone()]).await.unwrap();

        let context = SecurityContext::default().next(ActorId::Agent(agent.id), ActorId::None, ActorId::None);
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(EventContext::new(session.id, channel.id, ActorId::None, String::new()), None, context.clone(), channel.classification.clone(), Default::default()));
        let provider = ChannelLanguagePromptProvider::new(PromptPosition::Start, testing::prompts());
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
        assert!(prompt.text.contains("\"it\""), "without one of its own, the session's as it is now (issue 425): {}", prompt.text);
    }

    /// What `provider` tells agent `agent` in the turn begun in `state`.
    async fn told(provider: &ChannelLanguagePromptProvider, agent: AgentId, session_loop: &Arc<SessionLoop>, state: &mut AgentChannelState) -> Prompt {
        let turn = Turn::of(agent, "Jonathan".to_string(), state, AgentStateHandle::detached(), actor_runtime_in(&session_loop.channels()[0], ActorId::Agent(agent))).expect("a turn was begun");
        provider.get_prompt(&turn.situation()).await.unwrap()
    }
}
