//! The `channel_briefing` component (issue 245): tells the
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
//! An agent that keeps what it was told - an external one (issue 204) - is
//! told that what it was told of them no longer holds once neither says
//! anything (`PromptProvider::withdrawn`).

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::value::text::{Briefing, Summary};
use std::sync::Arc;

pub const CHANNEL_BRIEFING_KIND: &str = "channel_briefing";

/// Its texts in the prompt catalogue: the descriptions, and that what was
/// told of them no longer holds.
const TEXT: &str = "agents.channel_briefing";
const WITHDRAWN: &str = "agents.channel_briefing_withdrawn";

#[derive(Debug)]
pub struct ChannelBriefingComponent {
    position: PromptPosition,
    prompts: PromptTexts,
}

impl ChannelBriefingComponent {
    /// The descriptions at `position`.
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
            ("session", session.summary(), session.briefing()),
            ("channel", channel.summary(), channel.briefing()),
        ];
        // A session's descriptions are public; a channel's are the channel's.
        let classification = if says_nothing(&places[1]) {
            Classification::PUBLIC
        } else {
            channel.classification().clone()
        };
        Ok(Prompt::classified(render(&self.prompts, &places)?, classification))
    }

    fn withdrawn(&self) -> Result<Option<String>, ContextError> {
        Ok(Some(self.prompts.render(WITHDRAWN, ())?))
    }
}

/// A place - its kind, "session" or "channel", its summary and its
/// briefing.
type Place<'a> = (&'a str, Option<&'a Summary>, Option<&'a Briefing>);

/// Whether `place` has neither a summary nor a briefing.
fn says_nothing((_, summary, briefing): &Place<'_>) -> bool {
    summary.is_none() && briefing.is_none()
}

/// The parts of `prompts` describing each of `places` by its summary and
/// briefing, those that say nothing left out; empty if none says anything.
fn render(prompts: &PromptTexts, places: &[Place<'_>]) -> Result<String, PromptError> {
    let places: Vec<_> = places.iter()
        .filter(|place| !says_nothing(place))
        .map(|(kind, summary, briefing)| context! { kind, summary => summary.map(|s| s.as_str()), briefing => briefing.map(|b| b.as_str()) })
        .collect();
    if places.is_empty() {
        return Ok(String::new());
    }
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
    use caelumora_domain::value::name::Name;
    use caelumora_domain::work::Work;
    use caelumora_kernel::session::testing::participant_runtime_in;
    use caelumora_kernel::session::{SessionLoop, SessionManager};
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;

    #[test]
    fn a_place_is_described_by_its_summary_and_briefing() {
        let summary = Summary::new("Planning the release.").unwrap();
        let briefing = Briefing::new("Dates are in UTC.\nBe brief.").unwrap();
        let prompts = testing::prompts();
        assert_eq!(render(&prompts, &[("channel", Some(&summary), Some(&briefing))]).unwrap(),
            "[About this channel]\nWhat this channel is for, as its owner describes it.\nIn short: Planning the release.\n\nDates are in UTC.\nBe brief.\n");
        let release = Summary::new("Release 2.0").unwrap();
        assert_eq!(render(&prompts, &[("session", Some(&release), None)]).unwrap(), "[About this session]\nWhat this session is for, as its owner describes it.\nIn short: Release 2.0\n");
        assert_eq!(
            render(&prompts, &[("session", None, None)]).unwrap(),
            "",
            "a place that says nothing adds nothing"
        );
        assert_eq!(render(&prompts, &[("session", Some(&release), None), ("channel", None, Some(&briefing))]).unwrap(),
            "[About this session]\nWhat this session is for, as its owner describes it.\nIn short: Release 2.0\n\n\
             [About this channel]\nWhat this channel is for, as its owner describes it.\n\nDates are in UTC.\nBe brief.\n",
            "the parts a line apart");
        assert_eq!(render(&prompts, &[("session", None, None), ("channel", None, Some(&briefing))]).unwrap(),
            "[About this channel]\nWhat this channel is for, as its owner describes it.\n\nDates are in UTC.\nBe brief.\n",
            "no line before the first part");
    }

    #[test]
    fn what_was_told_of_them_is_withdrawn_as_no_longer_holding() {
        let provider = ChannelBriefingComponent::new(PromptPosition::Start, testing::prompts());
        let withdrawn = provider.withdrawn().unwrap();
        assert!(
            withdrawn.as_deref().is_some_and(|text| text.contains("no longer holds")),
            "the descriptions told before are taken back: {withdrawn:?}"
        );
    }

    #[tokio::test]
    async fn the_descriptions_are_read_at_every_turn_and_classified_as_their_place() {
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
            prompt.text.contains("In short: Release 2.0")
                && !prompt.text.contains("[About this channel]"),
            "{}",
            prompt.text
        );
        assert_eq!(
            prompt.classification,
            Some(Classification::PUBLIC),
            "a session's descriptions are public"
        );

        let running = session_loop.channel(channel.id).expect("the channel runs");
        running
            .update_description(&system, Some("Planning"), Some("Dates are in UTC."))
            .await
            .unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        let ours = prompt.text.find("[About this session]").unwrap();
        let its = prompt.text.find("[About this channel]").unwrap();
        assert!(ours < its, "the session's first: {}", prompt.text);
        assert!(
            prompt.text.contains("Dates are in UTC."),
            "the channel's as it is now: {}",
            prompt.text
        );
        assert_eq!(
            prompt.classification,
            Some(confidential),
            "a channel's are classified as the channel"
        );

        session_loop.update_description(&system, None, None).await.unwrap();
        let prompt = told(&provider, agent.id, &session_loop, &mut state).await;
        assert!(
            !prompt.text.contains("[About this session]"),
            "a session that says nothing any more: {}",
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
