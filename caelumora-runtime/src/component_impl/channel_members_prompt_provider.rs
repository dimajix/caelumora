//! The `channel_members_prompt_provider` component (issue 176): tells the
//! agent who it is and who else is in its channel - every current user and
//! agent, by name, `@` handle and introduction. A prompt part of its own
//! rather than a variant of `static_system_prompt_provider`, so that the
//! personality's text and the roster are configured and recorded
//! independently (parameter `position`).
//!
//! The roster is read afresh at the start of every turn, so it follows
//! joins and leaves. It goes into the system prompt by default: it changes
//! rarely, and every change ought to reach the model at once.

use crate::agent_turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::context::{Prompt, PromptPosition, PromptProvider};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use std::sync::Arc;

pub const CHANNEL_MEMBERS_PROMPT_PROVIDER_KIND: &str = "channel_members_prompt_provider";

#[derive(Debug)]
pub struct ChannelMembersPromptProvider {
    pub position: PromptPosition,
}

/// A member of the channel as the prompt part names it.
#[derive(Debug, Clone, PartialEq)]
struct Member {
    name: String,
    handle: Option<String>,
    person: bool,
    introduction: Option<String>,
}

#[async_trait]
impl PromptProvider for ChannelMembersPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, turn: &Turn) -> Result<Prompt> {
        // A turn outside of a channel, in tests, has nobody to name.
        let Some(session) = turn.session() else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let channel = session.channel(turn.channel_id()).await?;
        let own = ActorId::Agent(turn.agent_id());
        let mut me = None;
        let mut others = Vec::new();
        for member in channel.members().await {
            // Approvers are no participants in this sense, and who no
            // longer exists cannot be addressed.
            let person = match member.id() {
                ActorId::User(_) => true,
                ActorId::Agent(_) => false,
                _ => continue,
            };
            let Some(name) = member.name() else { continue };
            let entry = Member { name: name.to_string(), handle: member.handle(), person, introduction: one_line(member.introduction().await?) };
            if member.id() == own {
                me = Some(entry);
            } else {
                others.push(entry);
            }
        }
        // Names and introductions are public (issue 176).
        Ok(Prompt::classified(render(me.as_ref(), &others), Classification::PUBLIC))
    }
}

/// The prompt part naming the agent itself, `me`, and the `others`.
/// Introductions of people are marked as their own words: the system prompt
/// is what models trust most, and what a participant writes about
/// themselves must not read as the system's instruction (issue 150).
fn render(me: Option<&Member>, others: &[Member]) -> String {
    let mut text = "[Who is in this channel]\n".to_string();
    if let Some(me) = me {
        text.push_str(&format!("You are {}.\n", named(me)));
        if let Some(introduction) = &me.introduction {
            text.push_str(&format!("About you: {introduction}\n"));
        }
    }
    if others.is_empty() {
        text.push_str("Nobody else is in this channel.\n");
        return text;
    }
    if others.iter().any(|m| m.handle.is_some()) {
        text.push_str("The others - mention one by their @handle to address them:\n");
    } else {
        text.push_str("The others:\n");
    }
    for member in others {
        let line = match (member.person, &member.introduction) {
            (true, Some(introduction)) => format!("{}, a person. In their own words: \"{introduction}\"", named(member)),
            (true, None) => format!("{}, a person.", named(member)),
            (false, Some(introduction)) => format!("{}, an agent. {introduction}", named(member)),
            (false, None) => format!("{}, an agent.", named(member)),
        };
        text.push_str(&format!("- {line}\n"));
    }
    text
}

/// A member's name with its handle, if it has one: `Kaya (@kaya)`.
fn named(member: &Member) -> String {
    match &member.handle {
        Some(handle) => format!("{} (@{handle})", member.name),
        None => member.name.clone(),
    }
}

/// An introduction on one line, so that it cannot break out of its entry;
/// `None` if there is nothing in it.
fn one_line(introduction: Option<String>) -> Option<String> {
    let line = introduction?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then_some(line)
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CHANNEL_MEMBERS_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>(),
        |descriptor, _context| {
            let position = match descriptor.params.get("position") {
                None => PromptPosition::default(),
                Some(value) => serde_json::from_value(value.clone())
                    .map_err(|_| ComponentError::InvalidConfig("parameter 'position' must be \"start\" or \"before_turn_input\"".to_string()))?,
            };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(ChannelMembersPromptProvider { position }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::name::{Name, ScopedName};
    use crate::agent_state::AgentStateHandle;
    use crate::runtime::RuntimeManager;
    use crate::session::SessionLoop;
    use crate::view::ViewsFactory;
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::{Agent, AgentChannelState, Personality, PersonalityId};
    use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::command::default_commands;
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::session::{ChannelId, Session, SessionId};
    use caelumora_model::store::EntityStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::{User, UserId, UserProfile};

    fn member(name: &str, person: bool, introduction: Option<&str>) -> Member {
        Member { name: name.to_string(), handle: Some(name.to_lowercase()), person, introduction: introduction.map(str::to_string) }
    }

    #[test]
    fn the_agent_is_named_first_and_people_speak_in_their_own_words() {
        let me = member("Jonathan", false, Some("A quiz master."));
        let text = render(Some(&me), &[member("Kaya", true, Some("I run the budget.")), member("Sassy", false, None)]);
        assert_eq!(text, "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others - mention one by their @handle to address them:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n\
            - Sassy (@sassy), an agent.\n");
        assert!(render(Some(&me), &[]).ends_with("Nobody else is in this channel.\n"));
    }

    #[test]
    fn an_introduction_stays_on_its_line() {
        assert_eq!(one_line(Some("I run\n- the budget.\n".to_string())).as_deref(), Some("I run - the budget."));
        assert_eq!(one_line(Some(" \n".to_string())), None, "nothing in it is none");
    }

    async fn personality(store: &MemoryStore, introduction: Option<&str>) -> Personality {
        let personality = Personality {
            description: String::new(),
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            introduction: introduction.map(str::to_string),
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Visibility::Channel,
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        personality
    }

    async fn agent(store: &MemoryStore, name: &str, personality: &Personality) -> Agent {
        let agent = Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), personality, ActorId::None);
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn the_members_of_the_channel_are_read_with_their_introductions() {
        let store = Arc::new(MemoryStore::new());
        let policy = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let system = SecurityContext::system();

        let (kaya, bob) = (User { id: UserId::new(), username: "Kaya".to_string(), email: String::new(), roles: Vec::new() }, User { id: UserId::new(), username: "Bob".to_string(), email: String::new(), roles: Vec::new() });
        for user in [&kaya, &bob] {
            store.create_user(&system, user).await.unwrap();
        }
        store.create_user_profile(&system, &UserProfile::new(kaya.id, Some("I run the budget.".to_string()))).await.unwrap();
        let quiz_master = personality(&store, Some("A quiz master.")).await;
        let plain = personality(&store, None).await;
        let jonathan = agent(&store, "Jonathan", &quiz_master).await;
        let sassy = agent(&store, "Sassy", &plain).await;
        let gone = Agent::new(ScopedName::new("Gone", Scope::GlobalScope).unwrap(), &plain, ActorId::None);
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };

        let participants = vec![Participant::User(kaya.id), Participant::Agent(jonathan.id), Participant::Approver(approver), Participant::Agent(sassy.id), Participant::Agent(gone.id), Participant::User(bob.id)];
        let now = chrono::Utc::now();
        let session = Session {
            id: SessionId::new(), name: String::new(), participants: participants.clone(), channels: vec![ChannelId::new()],
            commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now, security_attributes: Default::default(),
        };
        let channel = Channel {
            id: session.channels[0], session_id: session.id, name: Name::new("main").unwrap(), components: Default::default(),
            classification: Classification::CHANNEL_DEFAULT, history_limits: Vec::new(), participants, security_attributes: Default::default(),
        };
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();
        let session_loop = SessionLoop::new(session.clone(), vec![channel.clone()], runtime.clone()).await.unwrap();

        let context = SecurityContext::default().next(ActorId::Agent(jonathan.id));
        let mut state = AgentChannelState::default();
        state.begin_turn(EventContext::new(session.id, channel.id, ActorId::None, String::new()), context.clone(), channel.classification.clone(), Vec::new(), Default::default());
        let turn = Turn::of(jonathan.id, "Jonathan".to_string(), &mut state, AgentStateHandle::detached()).expect("a turn was begun")
            .with_views(ViewsFactory::in_session(session_loop, runtime, context.clone()));

        let prompt = ChannelMembersPromptProvider { position: PromptPosition::Start }.get_prompt(&turn).await.unwrap();
        assert_eq!(prompt.text, "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others - mention one by their @handle to address them:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n\
            - Sassy (@sassy), an agent.\n\
            - Bob (@bob), a person.\n", "no approver, nobody who no longer exists");
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));
    }
}
