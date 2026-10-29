//! The `channel_participants_prompt_provider` component (issue 176): tells the
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
//! `changes_only`): the whole of it again. Who joined or left, by whose act,
//! it is told where it happened, as the channel's notice in what was said
//! (issues 408, 410). The component remembers what it told while it lives -
//! it is built for each actor, so for each channel - and tells all of it
//! again after a restart.

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, Prompt, PromptPosition, PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId};
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_model::value::text::Introduction;
use caelumora_runtime::component::{
    flag_of_params, ComponentFacets, ComponentKind, ComponentRegistry,
};
use std::sync::{Arc, Mutex};

pub const CHANNEL_PARTICIPANTS_PROMPT_PROVIDER_KIND: &str = "channel_participants_prompt_provider";

/// Its text in the prompt catalogue.
const TEXT: &str = "agents.channel_participants";

#[derive(Debug)]
pub struct ChannelParticipantsPromptProvider {
    pub position: PromptPosition,
    /// Whether it tells only of changes: then what it told last, `None`
    /// before it told anything. A plain lock, never held across an await.
    told: Option<Mutex<Option<String>>>,
    prompts: PromptTexts,
}

impl ChannelParticipantsPromptProvider {
    /// The roster at `position`, told every time it is asked.
    pub fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, told: None, prompts }
    }

    /// The roster at `position`, told the first time it is asked and then
    /// only when it changed (issue 204).
    pub fn changes_only(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, told: Some(Mutex::new(None)), prompts }
    }

    /// What the agent is to be told now, the roster rendered as `text`: all
    /// of it, or - telling only of changes - nothing if it is what it told
    /// last.
    fn text(&self, text: String) -> String {
        let Some(told) = &self.told else { return text };
        let mut told = told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if told.as_deref() == Some(text.as_str()) {
            return String::new();
        }
        *told = Some(text.clone());
        text
    }
}

#[async_trait]
impl PromptProvider for ChannelParticipantsPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // Work that reaches no session (`Situation::channel_view`) has nobody to name.
        let Some((me, others)) = participants(situation).await? else {
            return Ok(Prompt::classified(String::new(), Classification::PUBLIC));
        };
        let explanation = situation.profile().framing_explanation(&self.prompts)?;
        let text = self.text(render(&self.prompts, me.as_ref(), &others, explanation.as_deref())?);
        // Names and introductions are public (issue 176).
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }
}

/// The agent of `situation` and the others in its channel, each as the
/// views describe them, the agent `None` if it is not among them; `None`
/// for work that reaches no session. Who no longer exists is left out: it
/// cannot be addressed.
pub(crate) async fn participants(
    situation: &Situation<'_>,
) -> Result<Option<(Option<ActorDescription>, Vec<ActorDescription>)>, ContextError> {
    let Some(channel) = situation.channel_view().await? else { return Ok(None) };
    let own = ActorId::Agent(situation.agent_id());
    let mut me = None;
    let mut others = Vec::new();
    for participant in channel.participants().await {
        let Some(entry) = participant.describe().await? else { continue };
        if participant.id() == own {
            me = Some(entry);
        } else {
            others.push(entry);
        }
    }
    Ok(Some((me, others)))
}

/// The prompt part of `prompts` naming the agent itself, `me`, and the
/// `others`. Introductions of people are marked as their own words: the
/// system prompt is what models trust most, and what a participant writes
/// about themselves must not read as the system's instruction (issue 150).
/// Where an agent is among the others, it is told when to mention one, with
/// the first of them as the example (issues 438, 442). How the others'
/// messages are marked is told with them, `explanation`, as the agent's
/// model profile words it (issue 135).
fn render(
    prompts: &PromptTexts,
    me: Option<&ActorDescription>,
    others: &[ActorDescription],
    explanation: Option<&str>,
) -> Result<String, PromptError> {
    let described = |participant: &ActorDescription| {
        context! {
            name => participant.name.as_str(),
            handle => participant.handle(),
            person => participant.is_person(),
            introduction => participant.introduction.as_ref().map(one_line),
        }
    };
    let example =
        others.iter().find(|participant| !participant.is_person()).map(|agent| agent.name.as_str());
    prompts.render(TEXT, context! { me => me.map(described), others => others.iter().map(described).collect::<Vec<_>>(), example, explanation })
}

/// An introduction on one line, so that it cannot break out of its entry.
fn one_line(introduction: &Introduction) -> String {
    introduction.as_str().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CHANNEL_PARTICIPANTS_PROMPT_PROVIDER_KIND)
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let position = PromptPosition::of_params(&descriptor.params)?;
            let changes_only = flag_of_params(&descriptor.params, "changes_only")?;
            let provider = if changes_only {
                ChannelParticipantsPromptProvider::changes_only(position, prompts)
            } else {
                ChannelParticipantsPromptProvider::new(position, prompts)
            };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(provider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, TestAgents};
    use crate::turn::Turn;
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::testing::make_personality;
    use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_model::approval::{ApprovalStrategy, Approver};
    use caelumora_model::channel::{Channel, InternalActor, Participant};
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::session::Session;
    use caelumora_model::store::Persistence;
    use caelumora_model::user::{User, UserId, UserProfile};
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::work::Work;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use caelumora_runtime::session::testing::{actor_runtime_in, make_session};
    use caelumora_runtime::session::SessionManager;
    use caelumora_runtime::testing::TestRuntime;
    use caelumora_runtime::view::ViewError;
    use std::slice;

    fn participant(name: &str, person: bool, introduction: Option<&str>) -> ActorDescription {
        let id = if person { ActorId::User(UserId::new()) } else { ActorId::Agent(AgentId::new()) };
        ActorDescription {
            id,
            name: Name::new(name).unwrap(),
            introduction: introduction.map(|text| Introduction::new(text).unwrap()),
        }
    }

    #[test]
    fn the_agent_is_named_first_and_people_speak_in_their_own_words() {
        let me = participant("Jonathan", false, Some("A quiz master."));
        let others = [
            participant("Kaya", true, Some("I run the budget.")),
            participant("Sassy", false, None),
        ];
        let prompts = testing::prompts();
        let text = render(&prompts, Some(&me), &others, None).unwrap();
        assert_eq!(text, "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n\
            - Sassy (@sassy), an agent.\n\
            When to write an @handle, and when a name:\n\
            - @handle, or @all for all of them: only when you need that participant to answer you or to act, now. Mentioning an agent this way makes it stop and take a turn of its own.\n\
            - Name, without @: in everything else - when you greet, thank, praise, agree, answer, or speak about someone. \"Thanks, Sassy\", \"as Sassy said\".\n\
            - Check every @ before you send: if you do not need an answer or an action from that participant, write the name instead.\n");
        assert!(
            render(&prompts, Some(&me), &[], Some("Marked."))
                .unwrap()
                .ends_with("Nobody else is in this channel.\n"),
            "nothing to explain without others"
        );
        assert!(
            render(&prompts, Some(&me), &others, Some("Messages of the others begin ..."))
                .unwrap()
                .ends_with("write the name instead.\nMessages of the others begin ...\n"),
            "told with the others (issue 135)"
        );
        assert_eq!(
            render(&prompts, Some(&me), &others[..1], None).unwrap(),
            "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n",
            "among people alone, a mention asks nobody (issues 438, 442)"
        );
    }

    #[test]
    fn telling_only_of_changes_tells_the_roster_again_only_when_it_changed() {
        let changes = ChannelParticipantsPromptProvider::changes_only(
            PromptPosition::Start,
            testing::prompts(),
        );
        let prompts = testing::prompts();
        let roster = |others: &[ActorDescription]| render(&prompts, None, others, None).unwrap();
        let (kaya, bob) = (participant("Kaya", true, None), participant("Bob", true, None));
        let first = roster(slice::from_ref(&kaya));
        assert_eq!(changes.text(first.clone()), first, "told the first time");
        assert_eq!(changes.text(first.clone()), "", "not again unchanged");
        let both = roster(&[kaya, bob]);
        assert_eq!(
            changes.text(both.clone()),
            both,
            "all of it again, without an account of what changed (issue 410)"
        );
        assert_eq!(
            ChannelParticipantsPromptProvider::new(PromptPosition::Start, testing::prompts())
                .text(first.clone()),
            first,
            "told every time without it"
        );
    }

    #[test]
    fn an_introduction_stays_on_its_line() {
        assert_eq!(
            one_line(&Introduction::new("I run\n- the budget.\n").unwrap()),
            "I run - the budget."
        );
    }

    async fn personality(store: &Persistence, introduction: Option<&str>) -> Personality {
        // Named apart: a name is unique in its scope (issue 250).
        let id = PersonalityId::new();
        let personality = Personality {
            id,
            introduction: introduction.map(|text| Introduction::new(text).unwrap()),
            ..make_personality(&format!("plain {id}"))
        };
        store
            .entity_stores()
            .unwrap()
            .personalities
            .create(&SecurityContext::system(), &personality)
            .await
            .unwrap();
        personality
    }

    async fn agent(store: &Persistence, name: &str, personality: &Personality) -> Agent {
        let agent = Agent::new(
            ScopedName::new(name, Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(UserId::new()),
        );
        store
            .entity_stores()
            .unwrap()
            .agents
            .create(&SecurityContext::system(), &agent)
            .await
            .unwrap();
        agent
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn the_participants_of_the_channel_are_read_with_their_introductions() {
        let store = Persistence::in_memory();
        let runtime =
            TestRuntime { store: store.clone(), plugins: vec![&TestAgents], ..Default::default() }
                .build();
        let system = SecurityContext::system();

        let kaya = User {
            id: UserId::new(),
            username: Name::new("Kaya").unwrap(),
            email: String::new(),
            roles: Vec::new(),
            deleted: None,
            revision: Default::default(),
        };
        let bob = User {
            id: UserId::new(),
            username: Name::new("Bob").unwrap(),
            email: String::new(),
            roles: Vec::new(),
            deleted: None,
            revision: Default::default(),
        };
        for user in [&kaya, &bob] {
            store.entity_stores().unwrap().users.create(&system, user).await.unwrap();
        }
        store
            .entity_stores()
            .unwrap()
            .user_profiles
            .create(
                &system,
                &UserProfile::new(kaya.id, Some(Introduction::new("I run the budget.").unwrap())),
            )
            .await
            .unwrap();
        let quiz_master = personality(&store, Some("A quiz master.")).await;
        let plain = personality(&store, None).await;
        let jonathan = agent(&store, "Jonathan", &quiz_master).await;
        let sassy = agent(&store, "Sassy", &plain).await;
        let gone = agent(&store, "Gone", &plain).await;
        let approver = InternalActor::Approver(Approver::new(ApprovalStrategy::Any));

        let participants = vec![
            Participant::User(kaya.id),
            Participant::Agent(jonathan.id),
            Participant::Agent(sassy.id),
            Participant::Agent(gone.id),
            Participant::User(bob.id),
        ];
        let session = make_session("s", kaya.id);
        let mut channel = Channel::build(
            session.id,
            ActorId::User(kaya.id),
            Name::new("main").unwrap(),
            Default::default(),
            Classification::CHANNEL_DEFAULT,
            None,
        );
        (channel.participants, channel.internal_actors) = (participants.clone(), vec![approver]);
        let session = Session { participants, channels: vec![channel.id], ..session };
        let as_owner =
            SecurityContext::new(ActorId::User(kaya.id), Default::default(), Default::default());
        let session_loop = SessionManager::new(Arc::clone(&runtime))
            .create(&as_owner, session.clone(), vec![channel.clone()])
            .await
            .unwrap();
        // One who no longer exists: deleted once it founded the channel with the others.
        runtime.agent_manager().delete(&system, gone.id).await.unwrap();

        let context = SecurityContext::default().next(
            ActorId::Agent(jonathan.id),
            ActorId::None,
            ActorId::None,
        );
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            EventContext::new(session.id, channel.id, ActorId::None, String::new()),
            None,
            context.clone(),
            channel.classification.clone(),
            Default::default(),
        ));
        let turn = Turn::of(
            jonathan.id,
            "Jonathan".to_string(),
            &mut state,
            AgentStateHandle::detached(),
            actor_runtime_in(&session_loop.channels()[0], ActorId::Agent(jonathan.id)),
        )
        .expect("a turn was begun");

        let prompt =
            ChannelParticipantsPromptProvider::new(PromptPosition::Start, testing::prompts())
                .get_prompt(&turn.situation())
                .await
                .unwrap();
        assert_eq!(prompt.text, "[Who is in this channel]\n\
            You are Jonathan (@jonathan).\n\
            About you: A quiz master.\n\
            The others:\n\
            - Kaya (@kaya), a person. In their own words: \"I run the budget.\"\n\
            - Sassy (@sassy), an agent.\n\
            - Bob (@bob), a person.\n\
            When to write an @handle, and when a name:\n\
            - @handle, or @all for all of them: only when you need that participant to answer you or to act, now. Mentioning an agent this way makes it stop and take a turn of its own.\n\
            - Name, without @: in everything else - when you greet, thank, praise, agree, answer, or speak about someone. \"Thanks, Sassy\", \"as Sassy said\".\n\
            - Check every @ before you send: if you do not need an answer or an action from that participant, write the name instead.\n", "no approver, nobody who no longer exists");
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
        let context =
            SecurityContext::default().next(ActorId::Agent(outsider), ActorId::None, ActorId::None);
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            EventContext::new(session.id(), channel.id(), ActorId::None, String::new()),
            None,
            context,
            channel.classification(),
            Default::default(),
        ));
        let turn = Turn::of(
            outsider,
            "outsider".to_string(),
            &mut state,
            AgentStateHandle::detached(),
            actor_runtime_in(channel, ActorId::Agent(outsider)),
        )
        .expect("a turn was begun");
        assert!(
            matches!(turn.situation().session_view().await, Err(ViewError::SessionNotFound(_))),
            "reached, but not to be read (issue 399)"
        );
        let failed =
            ChannelParticipantsPromptProvider::new(PromptPosition::Start, testing::prompts())
                .get_prompt(&turn.situation())
                .await;
        assert!(
            matches!(failed, Err(ContextError::View(ViewError::SessionNotFound(_)))),
            "left to its fail policy: {failed:?}"
        );
    }
}
