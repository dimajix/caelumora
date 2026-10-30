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

use crate::prompt::{
    texts_of, writing_for_the_model, ChannelPartConfig, ContextError, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_model::value::text::{Briefing, Summary};
use caelumora_runtime::component::{
    config_of_params, ComponentFacets, ComponentKind, ComponentRegistry,
};
use std::sync::{Arc, Mutex};

pub const CHANNEL_BRIEFING_PROMPT_PROVIDER_KIND: &str = "channel_briefing_prompt_provider";

/// Its texts in the prompt catalogue: the descriptions, and that what was
/// told of them no longer holds.
const TEXT: &str = "agents.channel_briefing";
const WITHDRAWN: &str = "agents.channel_briefing_withdrawn";

#[derive(Debug)]
pub struct ChannelBriefingPromptProvider {
    pub position: PromptPosition,
    /// Whether it tells only of changes: then what it told last, `None`
    /// before it told anything. A plain lock, never held across an await.
    told: Option<Mutex<Option<String>>>,
    prompts: PromptTexts,
}

impl ChannelBriefingPromptProvider {
    /// The descriptions at `position`, told every time it is asked.
    pub fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, told: None, prompts }
    }

    /// The descriptions at `position`, told the first time it is asked and
    /// then only when they changed.
    pub fn changes_only(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, told: Some(Mutex::new(None)), prompts }
    }

    /// What the agent is to be told now, the descriptions rendered as
    /// `text`.
    fn text(&self, text: String) -> Result<String, PromptError> {
        let Some(told) = &self.told else { return Ok(text) };
        let mut told = told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match told.as_deref() {
            Some(before) if before == text => Ok(String::new()),
            // Nothing from the start: there is nothing to take back.
            None if text.is_empty() => {
                *told = Some(text);
                Ok(String::new())
            }
            Some(_) if text.is_empty() => {
                *told = Some(text);
                self.prompts.render(WITHDRAWN, ())
            }
            _ => {
                *told = Some(text.clone());
                Ok(text)
            }
        }
    }
}

#[async_trait]
impl PromptProvider for ChannelBriefingPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // Work that reaches no session (`Situation::channel_view`) has no place to describe.
        let Some(channel) = situation.channel_view().await? else {
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
        let text = self.text(render(&self.prompts, &places)?)?;
        Ok(Prompt::classified(text, classification))
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

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CHANNEL_BRIEFING_PROMPT_PROVIDER_KIND)
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let ChannelPartConfig { position, changes_only } =
                config_of_params(&descriptor.params)?;
            let provider = if changes_only {
                ChannelBriefingPromptProvider::changes_only(position, prompts)
            } else {
                ChannelBriefingPromptProvider::new(position, prompts)
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
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::agent::testing::{make_personality, stored_agent};
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::session::testing::make_session;
    use caelumora_model::session::Session;
    use caelumora_model::user::testing::{make_user, record_users};
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::Name;
    use caelumora_model::work::Work;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use caelumora_runtime::session::testing::actor_runtime_in;
    use caelumora_runtime::session::{SessionLoop, SessionManager};
    use caelumora_runtime::testing::TestRuntime;

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
    fn told_every_time_unless_only_changes_are_told() {
        let always = ChannelBriefingPromptProvider::new(PromptPosition::Start, testing::prompts());
        assert_eq!(
            always.text("a".to_string()).unwrap(),
            always.text("a".to_string()).unwrap(),
            "every turn"
        );

        let changes =
            ChannelBriefingPromptProvider::changes_only(PromptPosition::Start, testing::prompts());
        assert_eq!(
            changes.text(String::new()).unwrap(),
            "",
            "nothing from the start: nothing to tell"
        );
        assert_eq!(changes.text("a".to_string()).unwrap(), "a", "what is said first");
        assert_eq!(changes.text("a".to_string()).unwrap(), "", "not again while it stays");
        assert_eq!(changes.text("b".to_string()).unwrap(), "b", "but when it changes");
        assert!(
            changes.text(String::new()).unwrap().contains("no longer holds"),
            "and when it is taken back"
        );
        assert_eq!(changes.text(String::new()).unwrap(), "");
    }

    #[tokio::test]
    async fn the_descriptions_are_read_at_every_turn_and_classified_as_their_place() {
        let runtime = TestRuntime { plugins: vec![&TestAgents], ..Default::default() }.build();
        let system = SecurityContext::system();

        let agent = stored_agent(
            runtime.entity_stores(),
            "Jonathan",
            make_personality("plain"),
            ActorId::User(UserId::new()),
        )
        .await;
        let owner = make_user("owner");
        record_users(runtime.entity_stores(), &[&owner]).await;
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
        let session_loop = SessionManager::new(Arc::clone(&runtime))
            .create(&as_owner, session.clone(), vec![channel.clone()])
            .await
            .unwrap();

        let context =
            SecurityContext::default().next(ActorId::Agent(agent.id), ActorId::None, ActorId::None);
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            EventContext::new(session.id, channel.id, ActorId::None, String::new()),
            None,
            context.clone(),
            channel.classification.clone(),
            Default::default(),
        ));
        let provider =
            ChannelBriefingPromptProvider::new(PromptPosition::Start, testing::prompts());
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
        provider: &ChannelBriefingPromptProvider,
        agent: AgentId,
        session_loop: &Arc<SessionLoop>,
        state: &mut AgentChannelState,
    ) -> Prompt {
        let turn = Turn::of(
            agent,
            "Jonathan".to_string(),
            state,
            AgentStateHandle::detached(),
            actor_runtime_in(&session_loop.channels()[0], ActorId::Agent(agent)),
        )
        .expect("a turn was begun");
        provider.get_prompt(&turn.situation()).await.unwrap()
    }
}
