//! The `channel_briefing` component (issue 245): tells the agent where it works - its session and
//! its channel by their names (issue 575) and what they are for, their summaries and briefings as
//! whoever may change them wrote them -, so that it need not be told again in every request.
//!
//! Both are read afresh at the start of every turn through the agent's views, so that a change -
//! a new name too - reaches it with its next turn; the session's come first. Each part names its
//! place and gives its descriptions as written by its owner - not by whoever the agent talks
//! with -, and comes where the personality wires the component in, after its own instructions. In
//! the system prompt by default (parameter `position`): it changes rarely.
//!
//! A session's name and descriptions are public; a channel's are part of the channel, so the part
//! is classified as the channel is.

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::value::name::Name;
use caelumora_domain::value::text::{Briefing, Summary};
use std::sync::Arc;

pub const CHANNEL_BRIEFING_KIND: &str = "channel_briefing";

/// Its text in the prompt catalogue.
const TEXT: &str = "agents.channel_briefing";

#[derive(Debug)]
pub struct ChannelBriefingComponent {
    position: PromptPosition,
    prompts: PromptTexts,
}

impl ChannelBriefingComponent {
    /// The names and descriptions of the session and the channel, at `position`.
    pub(crate) fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, prompts }
    }
}

#[async_trait]
impl PromptProvider for ChannelBriefingComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // Work that reaches no session (`Situation::channel`) has no place to describe.
        let Some(channel) = situation.channel().await? else {
            return Ok(Prompt::classified(String::new(), Classification::PUBLIC));
        };
        let session = channel.session();
        let places = [
            PlaceTold {
                kind: "session",
                name: session.name(),
                summary: session.summary(),
                briefing: session.briefing(),
            },
            PlaceTold {
                kind: "channel",
                name: channel.name(),
                summary: channel.summary(),
                briefing: channel.briefing(),
            },
        ];
        let text = render(&self.prompts, &places)?;
        // A session's name and descriptions are public; a channel's are the channel's.
        Ok(Prompt::classified(text, channel.classification().clone()))
    }
}

/// A place the agent works in, as it is told of it.
struct PlaceTold<'a> {
    /// "session" or "channel".
    kind: &'a str,
    name: &'a Name,
    summary: Option<&'a Summary>,
    briefing: Option<&'a Briefing>,
}

/// The parts of `prompts` naming each of `places` and describing it by its summary and briefing,
/// where it has them.
fn render(prompts: &PromptTexts, places: &[PlaceTold<'_>]) -> Result<String, PromptError> {
    let places: Vec<_> = places
        .iter()
        .map(|place| {
            context! {
                kind => place.kind,
                name => place.name.as_str(),
                summary => place.summary.map(Summary::as_str),
                briefing => place.briefing.map(Briefing::as_str),
            }
        })
        .collect();
    prompts.render(TEXT, context! { places })
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CHANNEL_BRIEFING_KIND)
                .params::<PositionParams>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let position = PositionParams::position_of(&descriptor.params, PromptPosition::Start)?;
            let provider = ChannelBriefingComponent::new(position, prompts);
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
    use caelumora_domain::confidentiality::Confidentiality;
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserId;
    use caelumora_domain::work::Work;
    use caelumora_kernel::session::testing::participant_runtime_in;
    use caelumora_kernel::session::{SessionLoop, SessionManager};
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;

    #[test]
    fn a_place_is_named_and_described_by_its_summary_and_briefing() {
        let prompts = testing::prompts();
        let release = Name::new("release").unwrap();
        let main = Name::new("main").unwrap();
        let planning = Summary::new("Planning the release.").unwrap();
        let utc = Briefing::new("Dates are in UTC.\nBe brief.").unwrap();
        let session =
            |summary| PlaceTold { kind: "session", name: &release, summary, briefing: None };
        let channel =
            |briefing| PlaceTold { kind: "channel", name: &main, summary: None, briefing };

        let described = PlaceTold { summary: Some(&planning), ..channel(Some(&utc)) };
        assert_eq!(
            render(&prompts, &[described]).unwrap(),
            "[About this channel]\nThis channel is called \"main\".\n\
             What this channel is for, as its owner describes it.\nIn short: Planning the release.\n\n\
             Dates are in UTC.\nBe brief.\n",
            "a place named and described"
        );
        assert_eq!(
            render(&prompts, &[session(Some(&planning))]).unwrap(),
            "[About this session]\nThis session is called \"release\".\n\
             What this session is for, as its owner describes it.\nIn short: Planning the release.\n",
            "a place described in short alone"
        );
        assert_eq!(
            render(&prompts, &[session(None)]).unwrap(),
            "[About this session]\nThis session is called \"release\".\n",
            "a place that says nothing of itself is named all the same"
        );
        assert_eq!(
            render(&prompts, &[session(None), channel(Some(&utc))]).unwrap(),
            "[About this session]\nThis session is called \"release\".\n\n\
             [About this channel]\nThis channel is called \"main\".\n\
             What this channel is for, as its owner describes it.\n\nDates are in UTC.\nBe brief.\n",
            "the parts a line apart"
        );
    }

    #[tokio::test]
    async fn the_places_are_read_at_every_turn_and_classified_as_the_channel() {
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
        let confidential = Classification::general(Confidentiality::Confidential);
        let mut channel = Channel::build(
            session.id,
            ActorId::User(owner.id),
            Name::new("main").unwrap(),
            Default::default(),
            confidential.clone(),
            None,
        );
        channel.participants = vec![Participant::Agent(agent.id)];
        let session = Session {
            summary: Some(Summary::new("Release 2.0").unwrap()),
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
        let provider = ChannelBriefingComponent::new(PromptPosition::Start, testing::prompts());
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(
            prompt.text.contains("This session is called \"s\".")
                && prompt.text.contains("In short: Release 2.0")
                && prompt.text.contains("This channel is called \"main\"."),
            "both named, the session described: {}",
            prompt.text
        );
        assert_eq!(
            prompt.classification,
            Some(confidential.clone()),
            "the channel's name is classified as the channel"
        );

        let running = session_loop.channel(channel.id).expect("the channel runs");
        running
            .update_description(&system, Some("Planning"), Some("Dates are in UTC."))
            .await
            .unwrap();
        running.update_name(&system, "planning").await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        let ours = prompt.text.find("[About this session]").unwrap();
        let its = prompt.text.find("[About this channel]").unwrap();
        assert!(ours < its, "the session's first: {}", prompt.text);
        assert!(
            prompt.text.contains("This channel is called \"planning\".")
                && prompt.text.contains("Dates are in UTC."),
            "the channel as it is now: {}",
            prompt.text
        );

        session_loop.update_description(&system, None, None).await.unwrap();
        session_loop.update_name(&system, "release").await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(
            prompt.text.contains("This session is called \"release\".")
                && !prompt.text.contains("In short: Release 2.0"),
            "a session that says nothing of itself any more, by its name now: {}",
            prompt.text
        );
    }

    /// What `provider` tells agent `agent` in the turn begun in `state`.
    async fn told(
        provider: &ChannelBriefingComponent,
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
