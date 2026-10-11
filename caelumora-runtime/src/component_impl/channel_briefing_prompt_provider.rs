//! The `channel_briefing_prompt_provider` component (issue 245): tells the
//! agent what its session and its channel are for - their summaries and
//! briefings, as whoever may change them wrote them - so that it need not be
//! told again in every request.
//!
//! Both are read afresh at the start of every turn through the agent's
//! views, so that a change reaches it with its next turn; the session's
//! come first. Each part names itself as the description of its place,
//! written by its owner - not by whoever the agent talks with - and comes
//! where the personality wires the component in, after its own
//! instructions. In the system prompt by default (parameter `position`): it
//! changes rarely. A session and channel that say nothing add nothing.
//!
//! A session's descriptions are public, as its name is; a channel's are
//! part of the channel, and the part is classified as the channel is if it
//! holds them.
//!
//! An agent that keeps its conversation itself - an external one (issue
//! 204) - is told them once and then only when they changed (parameter
//! `changes_only`), as the roster and the language are.

use crate::component::{flag_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::context::{titled, Prompt, PromptPosition, PromptProvider};
use crate::agent::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::value::text::{Briefing, Summary};
use std::sync::{Arc, Mutex};

pub const CHANNEL_BRIEFING_PROMPT_PROVIDER_KIND: &str = "channel_briefing_prompt_provider";

#[derive(Debug)]
pub struct ChannelBriefingPromptProvider {
    pub position: PromptPosition,
    /// Whether it tells only of changes: then what it told last, `None`
    /// before it told anything. A plain lock, never held across an await.
    told: Option<Mutex<Option<String>>>,
}

impl ChannelBriefingPromptProvider {
    /// The descriptions at `position`, told every time it is asked.
    pub fn new(position: PromptPosition) -> Self {
        Self { position, told: None }
    }

    /// The descriptions at `position`, told the first time it is asked and
    /// then only when they changed.
    pub fn changes_only(position: PromptPosition) -> Self {
        Self { position, told: Some(Mutex::new(None)) }
    }

    /// What the agent is to be told now, the descriptions rendered as
    /// `text`.
    fn text(&self, text: String) -> String {
        let Some(told) = &self.told else { return text };
        let mut told = told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match told.as_deref() {
            Some(before) if before == text => String::new(),
            // Nothing from the start: there is nothing to take back.
            None if text.is_empty() => {
                *told = Some(text);
                String::new()
            }
            Some(_) if text.is_empty() => {
                *told = Some(text);
                titled("About this session and channel", "What you were told of them before no longer holds: neither says anything about itself now.")
            }
            _ => {
                *told = Some(text.clone());
                text
            }
        }
    }
}

#[async_trait]
impl PromptProvider for ChannelBriefingPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        // Work outside of a channel, in tests, has no place to describe.
        let Some(session) = work.session() else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let channel = session.channel(work.channel_id()).await?;
        let ours = described("session", session.summary(), session.briefing());
        let its = described("channel", channel.summary(), channel.briefing());
        // A session's descriptions are public; a channel's are the channel's.
        let classification = if its.is_empty() { Classification::PUBLIC } else { channel.classification().clone() };
        let text = self.text([ours, its].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n"));
        Ok(Prompt::classified(text, classification))
    }
}

/// The part describing the `place` - "session" or "channel" - by its
/// `summary` and `briefing`; empty if it has neither.
fn described(place: &str, summary: Option<&Summary>, briefing: Option<&Briefing>) -> String {
    if summary.is_none() && briefing.is_none() {
        return String::new();
    }
    let mut text = format!("What this {place} is for, as its owner describes it.\n");
    if let Some(summary) = summary {
        text.push_str(&format!("In short: {summary}\n"));
    }
    if let Some(briefing) = briefing {
        text.push_str(&format!("\n{briefing}\n"));
    }
    titled(&format!("About this {place}"), &text)
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CHANNEL_BRIEFING_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>(),
        |descriptor, _context| {
            let position = PromptPosition::of_params(&descriptor.params)?;
            let changes_only = flag_of_params(&descriptor.params, "changes_only")?;
            let provider = if changes_only { ChannelBriefingPromptProvider::changes_only(position) } else { ChannelBriefingPromptProvider::new(position) };
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
    use crate::view::ViewsFactory;
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::confidentiality::{Confidentiality, Visibility};
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::session::{Session, SessionId};
    use caelumora_model::store::EntityStore;
    use caelumora_model::store::memory::MemoryStore;

    #[test]
    fn a_place_is_described_by_its_summary_and_briefing() {
        let (summary, briefing) = (Summary::new("Planning the release.").unwrap(), Briefing::new("Dates are in UTC.\nBe brief.").unwrap());
        assert_eq!(described("channel", Some(&summary), Some(&briefing)),
            "[About this channel]\nWhat this channel is for, as its owner describes it.\nIn short: Planning the release.\n\nDates are in UTC.\nBe brief.\n");
        assert_eq!(described("session", Some(&Summary::new("Release 2.0").unwrap()), None), "[About this session]\nWhat this session is for, as its owner describes it.\nIn short: Release 2.0\n");
        assert_eq!(described("session", None, None), "", "a place that says nothing adds nothing");
    }

    #[test]
    fn told_every_time_unless_only_changes_are_told() {
        let always = ChannelBriefingPromptProvider::new(PromptPosition::Start);
        assert_eq!(always.text("a".to_string()), always.text("a".to_string()), "every turn");

        let changes = ChannelBriefingPromptProvider::changes_only(PromptPosition::Start);
        assert_eq!(changes.text(String::new()), "", "nothing from the start: nothing to tell");
        assert_eq!(changes.text("a".to_string()), "a", "what is said first");
        assert_eq!(changes.text("a".to_string()), "", "not again while it stays");
        assert_eq!(changes.text("b".to_string()), "b", "but when it changes");
        assert!(changes.text(String::new()).contains("no longer holds"), "and when it is taken back");
        assert_eq!(changes.text(String::new()), "");
    }

    #[tokio::test]
    async fn the_descriptions_are_read_at_every_turn_and_classified_as_their_place() {
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
        let confidential = Classification::general(Confidentiality::Confidential);
        let channel = Channel::build(session_id, ActorId::None, Name::new("main").unwrap(), Default::default(), confidential.clone(), None, vec![Participant::Agent(agent.id)]);
        let now = chrono::Utc::now();
        let session = Session {
            summary: Some(Summary::new("Release 2.0").unwrap()), briefing: None,
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
        let provider = ChannelBriefingPromptProvider::new(PromptPosition::Start);
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(prompt.text.contains("In short: Release 2.0") && !prompt.text.contains("[About this channel]"), "{}", prompt.text);
        assert_eq!(prompt.classification, Some(Classification::PUBLIC), "a session's descriptions are public");

        let running = session_loop.channel_by_id(channel.id).expect("the channel runs");
        running.update_description(&system, Some("Planning"), Some("Dates are in UTC.")).await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        let (ours, its) = (prompt.text.find("[About this session]").unwrap(), prompt.text.find("[About this channel]").unwrap());
        assert!(ours < its, "the session's first: {}", prompt.text);
        assert!(prompt.text.contains("Dates are in UTC."), "the channel's as it is now: {}", prompt.text);
        assert_eq!(prompt.classification, Some(confidential), "a channel's are classified as the channel");

        session_loop.update_description(&system, None, None).await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(!prompt.text.contains("[About this session]"), "a session that says nothing any more: {}", prompt.text);
    }

    /// What `provider` tells agent `agent` in the turn begun in `state`.
    async fn told(provider: &ChannelBriefingPromptProvider, agent: AgentId, session_loop: &Arc<SessionLoop>, state: &mut AgentChannelState) -> Prompt {
        let turn = Turn::of(agent, "Jonathan".to_string(), state, AgentStateHandle::detached()).expect("a turn was begun")
            .with_views(ViewsFactory::in_session(Arc::clone(session_loop)));
        provider.get_prompt(&turn.work()).await.unwrap()
    }
}
