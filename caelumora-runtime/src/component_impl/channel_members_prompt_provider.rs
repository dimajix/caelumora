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
//!
//! An agent that keeps its conversation itself - an external one (issue
//! 204) - is told the roster once and then only when it changed (parameter
//! `changes_only`): the whole of it again, with who joined and who left
//! since it was last told, which helps a model take a change in. The
//! component remembers what it told while it lives - it is built for each
//! actor, so for each channel - and tells all of it again after a restart.

use crate::component::{flag_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::context::{Prompt, PromptPosition, PromptProvider};
use crate::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use std::sync::{Arc, Mutex};

pub const CHANNEL_MEMBERS_PROMPT_PROVIDER_KIND: &str = "channel_members_prompt_provider";

#[derive(Debug)]
pub struct ChannelMembersPromptProvider {
    pub position: PromptPosition,
    /// Whether it tells only of changes: then the agent itself and the
    /// others it named last, `None` before it named any. A plain lock,
    /// never held across an await.
    told: Option<Mutex<Option<Roster>>>,
}

impl ChannelMembersPromptProvider {
    /// The roster at `position`, told every time it is asked.
    pub fn new(position: PromptPosition) -> Self {
        Self { position, told: None }
    }

    /// The roster at `position`, told the first time it is asked and then
    /// only when it changed, with the change (issue 204).
    pub fn changes_only(position: PromptPosition) -> Self {
        Self { position, told: Some(Mutex::new(None)) }
    }
}

/// The agent itself, if it is a member, and the others.
type Roster = (Option<Member>, Vec<Member>);

/// A member of the channel as the prompt part names it.
#[derive(Debug, Clone, PartialEq)]
struct Member {
    id: ActorId,
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

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        // Work outside of a channel, in tests, has nobody to name.
        let Some(session) = work.session() else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let channel = session.channel(work.channel_id()).await?;
        let own = ActorId::Agent(work.agent_id());
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
            let entry = Member { id: member.id(), name: name.to_string(), handle: member.handle(), person, introduction: one_line(member.introduction().await?) };
            if member.id() == own {
                me = Some(entry);
            } else {
                others.push(entry);
            }
        }
        let roster = (me, others);
        let explanation = work.profile().sender_explanation();
        let explanation = explanation.as_deref();
        let text = match &self.told {
            None => render(roster.0.as_ref(), &roster.1, explanation),
            Some(told) => {
                let mut told = told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if told.as_ref() == Some(&roster) {
                    String::new()
                } else {
                    let mut text = render(roster.0.as_ref(), &roster.1, explanation);
                    if let Some((_, before)) = told.as_ref() {
                        text.push_str(&render_changes(before, &roster.1));
                    }
                    *told = Some(roster);
                    text
                }
            }
        };
        // Names and introductions are public (issue 176).
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }
}

/// The prompt part naming the agent itself, `me`, and the `others`.
/// Introductions of people are marked as their own words: the system prompt
/// is what models trust most, and what a participant writes about
/// themselves must not read as the system's instruction (issue 150). How
/// the others' messages are marked is told with them, `explanation`, as
/// the agent's model profile words it (issue 135).
fn render(me: Option<&Member>, others: &[Member], explanation: Option<&str>) -> String {
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
            (true, Some(introduction)) => format!("{} In their own words: \"{introduction}\"", described(member)),
            (false, Some(introduction)) => format!("{} {introduction}", described(member)),
            (_, None) => described(member),
        };
        text.push_str(&format!("- {line}\n"));
    }
    if let Some(explanation) = explanation {
        text.push_str(&format!("{explanation}\n"));
    }
    text
}

/// Who of the others joined and who left since `before` was told, as an
/// addition to the roster `render` makes of `others`; empty if nobody did.
/// Told apart by who they are, not by their names, which may change.
fn render_changes(before: &[Member], others: &[Member]) -> String {
    let joined = others.iter().filter(|m| !before.iter().any(|b| b.id == m.id));
    let left = before.iter().filter(|b| !others.iter().any(|m| m.id == b.id));
    let lines: Vec<String> = joined.map(|m| format!("- Joined: {}\n", described(m)))
        .chain(left.map(|m| format!("- Left: {}\n", described(m))))
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!("Since you were last told who is here:\n{}", lines.concat())
}

/// A member with its handle and whether it is a person or an agent:
/// `Kaya (@kaya), a person.`
fn described(member: &Member) -> String {
    let kind = if member.person { "a person" } else { "an agent" };
    format!("{}, {kind}.", named(member))
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
            let position = PromptPosition::of_params(&descriptor.params)?;
            let changes_only = flag_of_params(&descriptor.params, "changes_only")?;
            let provider = if changes_only { ChannelMembersPromptProvider::changes_only(position) } else { ChannelMembersPromptProvider::new(position) };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(provider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_turn::Turn;
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
        Member { id: ActorId::None, name: name.to_string(), handle: Some(name.to_lowercase()), person, introduction: introduction.map(str::to_string) }
    }

    #[test]
    fn the_agent_is_named_first_and_people_speak_in_their_own_words() {
        let me = member("Jonathan", false, Some("A quiz master."));
        let others = [member("Kaya", true, Some("I run the budget.")), member("Sassy", false, None)];
        let text = render(Some(&me), &others, None);
        assert_eq!(text, "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others - mention one by their @handle to address them:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n\
            - Sassy (@sassy), an agent.\n");
        assert!(render(Some(&me), &[], Some("Marked.")).ends_with("Nobody else is in this channel.\n"), "nothing to explain without others");
        assert!(render(Some(&me), &others, Some("Messages of the others begin ...")).ends_with("- Sassy (@sassy), an agent.\nMessages of the others begin ...\n"), "told with the others (issue 135)");
    }

    #[test]
    fn who_joined_and_who_left_is_told_by_who_they_are() {
        let with_id = |name: &str, person: bool| Member { id: ActorId::User(UserId::new()), ..member(name, person, None) };
        let (kaya, sassy, bob) = (with_id("Kaya", true), with_id("Sassy", false), with_id("Bob", true));
        let renamed = Member { name: "Kaya K.".to_string(), ..kaya.clone() };
        assert_eq!(render_changes(&[kaya.clone(), sassy], &[renamed, bob]), "Since you were last told who is here:\n\
            - Joined: Bob (@bob), a person.\n\
            - Left: Sassy (@sassy), an agent.\n", "a new name is no new member");
        assert_eq!(render_changes(std::slice::from_ref(&kaya), std::slice::from_ref(&kaya)), "", "nobody came or went");
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
            deleted: None,
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
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let system = SecurityContext::system();

        let (kaya, bob) = (User { id: UserId::new(), username: "Kaya".to_string(), email: String::new(), roles: Vec::new(), deleted: None }, User { id: UserId::new(), username: "Bob".to_string(), email: String::new(), roles: Vec::new(), deleted: None });
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
            approval_rules: Vec::new(),
            id: SessionId::new(), name: String::new(), participants: participants.clone(), channels: vec![ChannelId::new()],
            commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now, security_attributes: Default::default(),
            deleted: None,
        };
        let channel = Channel {
            approval_rules: Vec::new(),
            id: session.channels[0], session_id: session.id, name: Name::new("main").unwrap(), components: Default::default(),
            classification: Classification::CHANNEL_DEFAULT, language: None, history_limits: Vec::new(), participants, security_attributes: Default::default(),
            deleted: None,
            former: Vec::new(),
        };
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();
        let session_loop = SessionLoop::new(session.clone(), vec![channel.clone()], Arc::clone(&runtime)).await.unwrap();

        let context = SecurityContext::default().next(ActorId::Agent(jonathan.id));
        let mut state = AgentChannelState::default();
        state.begin_turn(EventContext::new(session.id, channel.id, ActorId::None, String::new()), context.clone(), channel.classification.clone(), Vec::new(), Default::default());
        let turn = Turn::of(jonathan.id, "Jonathan".to_string(), &mut state, AgentStateHandle::detached()).expect("a turn was begun")
            .with_views(ViewsFactory::in_session(session_loop, context.clone()));

        let prompt = ChannelMembersPromptProvider::new(PromptPosition::Start).get_prompt(&turn.work()).await.unwrap();
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
