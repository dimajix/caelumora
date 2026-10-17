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

use caelumora_runtime::component::{flag_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::prompt::{ContextError, titled, Prompt, PromptPosition, PromptProvider};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId};
use caelumora_model::confidentiality::Classification;
use caelumora_model::value::text::Introduction;
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

/// The agent itself, if it is a member, and the others, as they are
/// described to everyone in the channel (issue 403).
type Roster = (Option<ActorDescription>, Vec<ActorDescription>);

#[async_trait]
impl PromptProvider for ChannelMembersPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // Work that reaches no session (`Situation::channel`) has nobody to name.
        let Some(channel) = situation.channel().await? else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let own = ActorId::Agent(situation.agent_id());
        let mut me = None;
        let mut others = Vec::new();
        for member in channel.members().await {
            // Approvers are no participants in this sense, and who no
            // longer exists cannot be addressed.
            if !matches!(member.id(), ActorId::User(_) | ActorId::Agent(_)) {
                continue;
            }
            let Some(entry) = member.describe().await? else { continue };
            if member.id() == own {
                me = Some(entry);
            } else {
                others.push(entry);
            }
        }
        let roster = (me, others);
        let explanation = situation.profile().sender_explanation();
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
fn render(me: Option<&ActorDescription>, others: &[ActorDescription], explanation: Option<&str>) -> String {
    let mut text = String::new();
    if let Some(me) = me {
        text.push_str(&format!("You are {}.\n", named(me)));
        if let Some(introduction) = me.introduction.as_ref().map(one_line) {
            text.push_str(&format!("About you: {introduction}\n"));
        }
    }
    if others.is_empty() {
        text.push_str("Nobody else is in this channel.\n");
        return titled("Who is in this channel", &text);
    }
    // `@all` addresses every one of them, with a handle or without (issue 401).
    if others.iter().any(|m| m.handle().is_some()) {
        text.push_str("The others - mention one by their @handle to address them, or all of them with @all:\n");
    } else {
        text.push_str("The others - mention all of them with @all to address them:\n");
    }
    for member in others {
        let line = match (member.is_person(), member.introduction.as_ref().map(one_line)) {
            (true, Some(introduction)) => format!("{} In their own words: \"{introduction}\"", described(member)),
            (false, Some(introduction)) => format!("{} {introduction}", described(member)),
            (_, None) => described(member),
        };
        text.push_str(&format!("- {line}\n"));
    }
    if let Some(explanation) = explanation {
        text.push_str(&format!("{explanation}\n"));
    }
    titled("Who is in this channel", &text)
}

/// Who of the others joined and who left since `before` was told, as an
/// addition to the roster `render` makes of `others`; empty if nobody did.
/// Told apart by who they are, not by their names, which may change.
fn render_changes(before: &[ActorDescription], others: &[ActorDescription]) -> String {
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
fn described(member: &ActorDescription) -> String {
    let kind = if member.is_person() { "a person" } else { "an agent" };
    format!("{}, {kind}.", named(member))
}

/// A member's name with its handle, if it has one: `Kaya (@kaya)`.
fn named(member: &ActorDescription) -> String {
    match member.handle() {
        Some(handle) => format!("{} (@{handle})", member.name),
        None => member.name.to_string(),
    }
}

/// An introduction on one line, so that it cannot break out of its entry.
fn one_line(introduction: &Introduction) -> String {
    introduction.as_str().split_whitespace().collect::<Vec<_>>().join(" ")
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
    use crate::turn::Turn;
    use crate::testing::{self, TestAgents};
    use caelumora_runtime::view::ViewError;
    use caelumora_runtime::testing::TestRuntime;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_runtime::agent::state::AgentStateHandle;
    use caelumora_runtime::session::SessionLoop;
    use caelumora_runtime::session::testing::actor_runtime_in;
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::session::{ChannelId, Session, SessionId};
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::{User, UserId, UserProfile};
    use caelumora_model::value::text::Introduction;
    use chrono::Utc;
    use uuid::Uuid;

    fn member(name: &str, person: bool, introduction: Option<&str>) -> ActorDescription {
        let id = if person { ActorId::User(UserId::new()) } else { ActorId::Agent(AgentId::new()) };
        ActorDescription { id, name: Name::new(name).unwrap(), introduction: introduction.map(|text| Introduction::new(text).unwrap()) }
    }

    #[test]
    fn the_agent_is_named_first_and_people_speak_in_their_own_words() {
        let me = member("Jonathan", false, Some("A quiz master."));
        let others = [member("Kaya", true, Some("I run the budget.")), member("Sassy", false, None)];
        let text = render(Some(&me), &others, None);
        assert_eq!(text, "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others - mention one by their @handle to address them, or all of them with @all:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n\
            - Sassy (@sassy), an agent.\n");
        assert!(render(Some(&me), &[], Some("Marked.")).ends_with("Nobody else is in this channel.\n"), "nothing to explain without others");
        assert!(render(Some(&me), &others, Some("Messages of the others begin ...")).ends_with("- Sassy (@sassy), an agent.\nMessages of the others begin ...\n"), "told with the others (issue 135)");
    }

    #[test]
    fn who_joined_and_who_left_is_told_by_who_they_are() {
        let (kaya, sassy, bob) = (member("Kaya", true, None), member("Sassy", false, None), member("Bob", true, None));
        let renamed = ActorDescription { name: Name::new("Kaya K.").unwrap(), ..kaya.clone() };
        assert_eq!(render_changes(&[kaya.clone(), sassy], &[renamed, bob]), "Since you were last told who is here:\n\
            - Joined: Bob (@bob), a person.\n\
            - Left: Sassy (@sassy), an agent.\n", "a new name is no new member");
        assert_eq!(render_changes(std::slice::from_ref(&kaya), std::slice::from_ref(&kaya)), "", "nobody came or went");
    }

    #[test]
    fn an_introduction_stays_on_its_line() {
        assert_eq!(one_line(&Introduction::new("I run\n- the budget.\n").unwrap()), "I run - the budget.");
    }

    async fn personality(store: &Arc<MemoryStore>, introduction: Option<&str>) -> Personality {
        // Named apart: a name is unique in its scope (issue 250).
        let id = PersonalityId::new();
        let personality = Personality {
            description: None,
            id,
            name: ScopedName::new(format!("plain {id}"), Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            introduction: introduction.map(|text| Introduction::new(text).unwrap()),
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Visibility::Channel,
            deleted: None,
        };
        store.entity_stores().personalities.create(&SecurityContext::system(), &personality).await.unwrap();
        personality
    }

    async fn agent(store: &Arc<MemoryStore>, name: &str, personality: &Personality) -> Agent {
        let agent = Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), personality.id, ActorId::None);
        store.entity_stores().agents.create(&SecurityContext::system(), &agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn the_members_of_the_channel_are_read_with_their_introductions() {
        let store = Arc::new(MemoryStore::new());
        let runtime = TestRuntime { store: Arc::clone(&store), plugins: vec![&TestAgents], ..Default::default() }.build();
        let system = SecurityContext::system();

        let (kaya, bob) = (User { id: UserId::new(), username: Name::new("Kaya").unwrap(), email: String::new(), roles: Vec::new(), deleted: None }, User { id: UserId::new(), username: Name::new("Bob").unwrap(), email: String::new(), roles: Vec::new(), deleted: None });
        for user in [&kaya, &bob] {
            store.entity_stores().users.create(&system, user).await.unwrap();
        }
        store.entity_stores().user_profiles.create(&system, &UserProfile::new(kaya.id, Some(Introduction::new("I run the budget.").unwrap()))).await.unwrap();
        let quiz_master = personality(&store, Some("A quiz master.")).await;
        let plain = personality(&store, None).await;
        let jonathan = agent(&store, "Jonathan", &quiz_master).await;
        let sassy = agent(&store, "Sassy", &plain).await;
        let gone = Agent::new(ScopedName::new("Gone", Scope::GlobalScope).unwrap(), plain.id, ActorId::None);
        let approver = Approver { id: ApproverId(Uuid::new_v4()), strategy: ApprovalStrategy::Any };

        let participants = vec![Participant::User(kaya.id), Participant::Agent(jonathan.id), Participant::Approver(approver), Participant::Agent(sassy.id), Participant::Agent(gone.id), Participant::User(bob.id)];
        let now = Utc::now();
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionId::new(), name: Name::new("s").unwrap(), participants: participants.clone(), channels: vec![ChannelId::new()],
            commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now, security_attributes: Default::default(),
            deleted: None,
        };
        let channel = Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            uses_workspace: true,
            id: session.channels[0], session_id: session.id, name: Name::new("main").unwrap(), components: Default::default(),
            classification: Classification::CHANNEL_DEFAULT, language: None, history_limits: Vec::new(), participants, security_attributes: Default::default(),
            deleted: None,
            former: Vec::new(),
        };
        store.entity_stores().sessions.create(&system, &session).await.unwrap();
        store.entity_stores().channels.create(&system, &channel).await.unwrap();
        let session_loop = SessionLoop::new(session.clone(), vec![channel.clone()], Arc::clone(&runtime)).await.unwrap();

        let context = SecurityContext::default().next(ActorId::Agent(jonathan.id), ActorId::None, ActorId::None);
        let mut state = AgentChannelState::default();
        state.begin_turn(EventContext::new(session.id, channel.id, ActorId::None, String::new()), context.clone(), channel.classification.clone(), Default::default());
        let turn = Turn::of(jonathan.id, "Jonathan".to_string(), &mut state, AgentStateHandle::detached(), actor_runtime_in(&session_loop.channels()[0], ActorId::Agent(jonathan.id))).expect("a turn was begun");

        let prompt = ChannelMembersPromptProvider::new(PromptPosition::Start).get_prompt(&turn.situation()).await.unwrap();
        assert_eq!(prompt.text, "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others - mention one by their @handle to address them, or all of them with @all:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n\
            - Sassy (@sassy), an agent.\n\
            - Bob (@bob), a person.\n", "no approver, nobody who no longer exists");
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));
    }

    #[tokio::test]
    async fn work_that_may_not_read_its_session_fails_the_part_rather_than_naming_nobody() {
        let world = testing::world().await;
        let personality = world.personality("plain", None, &[]).await;
        let outsider = world.agent("outsider", world.alice, &personality).await;
        let session = world.shared_session(&[], &[]).await;
        let channel = &session.channels()[0];
        // In its own name alone: nobody it acts for may read the session.
        let context = SecurityContext::default().next(ActorId::Agent(outsider), ActorId::None, ActorId::None);
        let mut state = AgentChannelState::default();
        state.begin_turn(EventContext::new(session.id(), channel.id(), ActorId::None, String::new()), context, channel.classification(), Default::default());
        let turn = Turn::of(outsider, "outsider".to_string(), &mut state, AgentStateHandle::detached(), actor_runtime_in(channel, ActorId::Agent(outsider))).expect("a turn was begun");
        assert!(matches!(turn.situation().session().await, Err(ViewError::SessionNotFound(_))), "reached, but not to be read (issue 399)");
        let failed = ChannelMembersPromptProvider::new(PromptPosition::Start).get_prompt(&turn.situation()).await;
        assert!(matches!(failed, Err(ContextError::View(ViewError::SessionNotFound(_)))), "left to its fail policy: {failed:?}");
    }
}
