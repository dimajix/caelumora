//! The `channel_language_prompt_provider` component (issue 56): tells the
//! agent which language its channel's conversation is held in, so that it
//! writes in it - although its instructions, its tools and the parts the
//! runtime adds are in English, which pulls a model towards English.
//! Nothing is translated: the conversation is one, in one language. The
//! language is named in English and in itself, and by its tag.
//!
//! The language is a property of the channel, read afresh at the start of
//! every turn through the agent's views, so that a change reaches it with
//! its next turn. It goes into the system prompt by default (parameter
//! `position`): it changes rarely. A channel held in no language in
//! particular adds nothing. A personality without the component leaves its
//! agents to choose themselves.
//!
//! An agent that keeps its conversation itself - an external one (issue
//! 204) - is told the language once and then only when it changed
//! (parameter `changes_only`), as the roster is
//! (`channel_members_prompt_provider`).

use crate::component::{flag_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::context::{titled, Prompt, PromptPosition, PromptProvider};
use crate::agent::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::value::language::Language;
use std::sync::{Arc, Mutex};

pub const CHANNEL_LANGUAGE_PROMPT_PROVIDER_KIND: &str = "channel_language_prompt_provider";

/// The title of its part.
const TITLE: &str = "Language of this channel";

#[derive(Debug)]
pub struct ChannelLanguagePromptProvider {
    pub position: PromptPosition,
    /// Whether it tells only of changes: then the language it told last,
    /// `None` before it told any. A plain lock, never held across an await.
    told: Option<Mutex<Option<Option<Language>>>>,
}

impl ChannelLanguagePromptProvider {
    /// The language at `position`, told every time it is asked.
    pub fn new(position: PromptPosition) -> Self {
        Self { position, told: None }
    }

    /// The language at `position`, told the first time it is asked and then
    /// only when it changed.
    pub fn changes_only(position: PromptPosition) -> Self {
        Self { position, told: Some(Mutex::new(None)) }
    }

    /// What the agent is to be told of `language` now.
    fn text(&self, language: Option<Language>) -> String {
        let Some(told) = &self.told else { return render(language.as_ref()) };
        let mut told = told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match told.as_ref() {
            Some(before) if *before == language => String::new(),
            // Held in none from the start: there is nothing to take back.
            None if language.is_none() => {
                *told = Some(None);
                String::new()
            }
            Some(Some(_)) if language.is_none() => {
                *told = Some(None);
                titled(TITLE, "This channel's conversation is no longer held in a particular language.")
            }
            _ => {
                let text = render(language.as_ref());
                *told = Some(language);
                text
            }
        }
    }
}

#[async_trait]
impl PromptProvider for ChannelLanguagePromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        // Work that reaches no session (`AgentWork::session`) has no language to follow.
        let Some(session) = work.session().await else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let channel = session.channel(work.channel_id()).await?;
        let text = self.text(channel.language().cloned());
        // Which language a channel is held in is no secret of its contents.
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }
}

/// The prompt part telling the agent to write in `language`; empty if the
/// channel is held in none in particular. The language is named in English
/// and in itself where `Language::names` knows it - a model follows a name
/// more surely than a tag - and always by its tag, which says the region.
fn render(language: Option<&Language>) -> String {
    let Some(language) = language else { return String::new() };
    let (held_in, write_in) = match language.names() {
        Some((english, native)) if english == native => (format!("{english} (BCP 47 tag \"{language}\")"), english.to_string()),
        Some((english, native)) => (format!("{english} - {native}, BCP 47 tag \"{language}\""), english.to_string()),
        None => (format!("the language with the BCP 47 tag \"{language}\""), "it".to_string()),
    };
    titled(TITLE, &format!("This channel's conversation is held in {held_in}. \
        Write everything in {write_in} - your answers, your messages to the others and what you note down - \
        whatever language your instructions, your tools or earlier messages are in."))
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CHANNEL_LANGUAGE_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>(),
        |descriptor, _context| {
            let position = PromptPosition::of_params(&descriptor.params)?;
            let changes_only = flag_of_params(&descriptor.params, "changes_only")?;
            let provider = if changes_only { ChannelLanguagePromptProvider::changes_only(position) } else { ChannelLanguagePromptProvider::new(position) };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(provider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::state::AgentStateHandle;
    use crate::agent::turn::Turn;
    use crate::runtime::RuntimeManager;
    use crate::session::SessionLoop;
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::session::{Session, SessionId};
    use caelumora_model::store::Entities;
    use caelumora_model::store::memory::MemoryStore;

    fn language(tag: &str) -> Option<Language> {
        Some(Language::new(tag).unwrap())
    }

    #[test]
    fn the_agent_is_told_the_language_by_its_names_and_its_tag() {
        assert_eq!(render(language("de-CH").as_ref()), "[Language of this channel]\n\
            This channel's conversation is held in German - Deutsch, BCP 47 tag \"de-CH\". \
            Write everything in German - your answers, your messages to the others and what you note down - \
            whatever language your instructions, your tools or earlier messages are in.\n");
        assert!(render(language("en").as_ref()).contains("held in English (BCP 47 tag \"en\"). Write everything in English"), "a name once if it is the same");
        let unknown = render(language("tlh").as_ref());
        assert!(unknown.contains("held in the language with the BCP 47 tag \"tlh\". Write everything in it"), "{unknown}");
        assert_eq!(render(None), "", "a channel held in no language adds nothing");
    }

    #[test]
    fn told_every_time_unless_only_changes_are_told() {
        let always = ChannelLanguagePromptProvider::new(PromptPosition::Start);
        assert_eq!(always.text(language("de")), always.text(language("de")), "every turn");

        let changes = ChannelLanguagePromptProvider::changes_only(PromptPosition::Start);
        assert_eq!(changes.text(None), "", "held in none from the start: nothing to tell");
        assert!(changes.text(language("de")).contains("\"de\""), "the first language");
        assert_eq!(changes.text(language("de")), "", "not again while it stays");
        assert!(changes.text(language("fr")).contains("\"fr\""), "but when it changes");
        assert!(changes.text(None).contains("no longer held in a particular language"), "and when it is taken back");
        assert_eq!(changes.text(None), "");
    }

    #[tokio::test]
    async fn the_language_is_read_from_the_channel_at_every_turn() {
        let store = Arc::new(MemoryStore::new());
        let policy = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::new(caelumora_model::confidentiality::ConfidentialityRules::default()), &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let system = SecurityContext::system();

        let personality = Personality {
            description: None, id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Visibility::Channel, deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let agent = Agent::new(ScopedName::new("Jonathan", Scope::GlobalScope).unwrap(), &personality, ActorId::None);
        store.create_agent(&system, &agent).await.unwrap();
        let session_id = SessionId::new();
        let channel = Channel::build(session_id, ActorId::None, Name::new("main").unwrap(), Default::default(), Classification::CHANNEL_DEFAULT, language("de"), vec![Participant::Agent(agent.id)]);
        let now = chrono::Utc::now();
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: session_id, name: caelumora_model::value::name::Name::new("s").unwrap(), participants: channel.participants.clone(), channels: vec![channel.id],
            commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now, security_attributes: Default::default(),
            deleted: None,
        };
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();
        let session_loop = SessionLoop::new(session.clone(), vec![channel.clone()], Arc::clone(&runtime)).await.unwrap();

        let context = SecurityContext::default().next(ActorId::Agent(agent.id), ActorId::None);
        let mut state = AgentChannelState::default();
        state.begin_turn(EventContext::new(session.id, channel.id, ActorId::None, String::new()), context.clone(), channel.classification.clone(), Vec::new(), Default::default());
        let provider = ChannelLanguagePromptProvider::new(PromptPosition::Start);
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(prompt.text.contains("\"de\""), "{}", prompt.text);
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));

        let running = session_loop.channel_by_id(channel.id).expect("the channel runs");
        running.update_language(&system, Some("fr")).await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(prompt.text.contains("\"fr\""), "the language as it is now: {}", prompt.text);
    }

    /// What `provider` tells agent `agent` in the turn begun in `state`.
    async fn told(provider: &ChannelLanguagePromptProvider, agent: AgentId, session_loop: &Arc<SessionLoop>, state: &mut AgentChannelState) -> Prompt {
        let turn = Turn::of(agent, "Jonathan".to_string(), state, AgentStateHandle::detached(), crate::session::testing::views_in(session_loop, &session_loop.channels()[0], ActorId::Agent(agent))).expect("a turn was begun");
        provider.get_prompt(&turn.work()).await.unwrap()
    }
}
