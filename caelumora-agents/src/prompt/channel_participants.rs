//! The `channel_participants` component (issue 176): tells the
//! agent who it is and who else is in its channel - every current user and
//! agent, by name, `@` handle and introduction. A prompt part of its own
//! rather than a variant of `system_prompt`, so that the
//! personality's text and the roster are configured and recorded
//! independently (parameter `position`).
//!
//! The roster is read afresh at the start of every turn, so it follows
//! joins and leaves. It goes into the system prompt by default: it changes
//! rarely, and every change ought to reach the model at once.
//!
//! An agent that keeps what it was told - an external one (issue 204) - is
//! told the roster again only when it changed, by its actor: the whole of it
//! again. Who joined or left, by whose act, it is told where it happened, as
//! the channel's notice in what was said (issues 408, 410).

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::actor::{ActorDescription, ActorId};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::value::text::Introduction;
use std::sync::Arc;

pub const CHANNEL_PARTICIPANTS_KIND: &str = "channel_participants";

/// Its text in the prompt catalogue.
const TEXT: &str = "agents.channel_participants";

#[derive(Debug)]
pub struct ChannelParticipantsComponent {
    position: PromptPosition,
    prompts: PromptTexts,
}

impl ChannelParticipantsComponent {
    /// The roster at `position`.
    pub(crate) fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, prompts }
    }
}

#[async_trait]
impl PromptProvider for ChannelParticipantsComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // Work that reaches no session (`Situation::channel`) has nobody to name.
        let Some((me, others)) = participants(situation).await? else {
            return Ok(Prompt::classified(String::new(), Classification::PUBLIC));
        };
        let explanation = situation.profile().framing_explanation(&self.prompts)?;
        let text = render(&self.prompts, me.as_ref(), &others, explanation.as_deref())?;
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
    let Some(channel) = situation.channel().await? else { return Ok(None) };
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

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CHANNEL_PARTICIPANTS_KIND)
                .params::<PositionParams>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let position = PositionParams::position_of(&descriptor.params, PromptPosition::Start)?;
            let provider = ChannelParticipantsComponent::new(position, prompts);
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
    use caelumora_api::reader::ViewError;
    use caelumora_domain::actor::EventContext;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_domain::approval::{ApprovalStrategy, Approver};
    use caelumora_domain::channel::{Channel, InternalActor, Participant};
    use caelumora_domain::scope::Scope;
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::{UserId, UserProfile};
    use caelumora_domain::value::name::{Name, ScopedName};
    use caelumora_domain::work::Work;
    use caelumora_kernel::session::testing::participant_runtime_in;
    use caelumora_kernel::session::SessionManager;
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::{Policies, SecurityContext};
    use caelumora_store::testing::record_users;
    use caelumora_store::{EntityStores, Persistence};

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
    fn an_introduction_stays_on_its_line() {
        assert_eq!(
            one_line(&Introduction::new("I run\n- the budget.\n").unwrap()),
            "I run - the budget."
        );
    }

    async fn personality(store: &Persistence, introduction: Option<&str>) -> Personality {
        // Named apart: a name is unique in its scope (issue 250).
        let id = PersonalityId::new();
        let mut personality = Personality {
            id,
            introduction: introduction.map(|text| Introduction::new(text).unwrap()),
            ..make_personality(&format!("plain {id}"))
        };
        EntityStores::open(store, &Policies::permitting_all())
            .unwrap()
            .personalities
            .create(&SecurityContext::system(), &mut personality)
            .await
            .unwrap();
        personality
    }

    async fn agent(store: &Persistence, name: &str, personality: &Personality) -> Agent {
        let mut agent = Agent::new(
            ScopedName::new(name, Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(UserId::new()),
        );
        EntityStores::open(store, &Policies::permitting_all())
            .unwrap()
            .agents
            .create(&SecurityContext::system(), &mut agent)
            .await
            .unwrap();
        agent
    }

    #[tokio::test]
    async fn the_participants_of_the_channel_are_read_with_their_introductions() {
        let store = Persistence::in_memory();
        let kernel = TestKernel {
            store: store.clone(),
            libraries: vec![&AgentsLibrary],
            ..Default::default()
        }
        .build();
        let system = SecurityContext::system();

        let kaya = make_user("Kaya");
        let bob = make_user("Bob");
        record_users(kernel.entity_stores(), &[&kaya, &bob]).await;
        kernel
            .entity_stores()
            .user_profiles
            .create(
                &system,
                &mut UserProfile::new(
                    kaya.id,
                    Some(Introduction::new("I run the budget.").unwrap()),
                ),
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
        let session_loop = SessionManager::new(Arc::clone(&kernel))
            .create(&as_owner, session.clone(), vec![channel.clone()])
            .await
            .unwrap();
        // One who no longer exists: deleted once it founded the channel with the others.
        kernel.agent_manager().delete(&system, gone.id).await.unwrap();
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            EventContext::new(session.id, channel.id, ActorId::None, String::new()),
            None,
            channel.classification.clone(),
            Default::default(),
        ));
        let turn = Turn::of(
            &mut state,
            agent_runtime(participant_runtime_in(
                &session_loop.channels()[0],
                ActorId::Agent(jonathan.id),
            )),
        )
        .expect("a turn was begun");

        let prompt = ChannelParticipantsComponent::new(PromptPosition::Start, testing::prompts())
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
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            EventContext::new(session.id(), channel.id(), ActorId::None, String::new()),
            None,
            channel.classification(),
            Default::default(),
        ));
        let turn = Turn::of(
            &mut state,
            agent_runtime(participant_runtime_in(channel, ActorId::Agent(outsider))),
        )
        .expect("a turn was begun");
        assert!(
            matches!(turn.situation().session().await, Err(ViewError::SessionNotFound(_))),
            "reached, but not to be read (issue 399)"
        );
        let failed = ChannelParticipantsComponent::new(PromptPosition::Start, testing::prompts())
            .get_prompt(&turn.situation())
            .await;
        assert!(
            matches!(failed, Err(ContextError::View(ViewError::SessionNotFound(_)))),
            "left to its fail policy: {failed:?}"
        );
    }
}
