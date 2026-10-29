//! What the model of a turn is given (issues 47, 49, 63, 136, 137): the
//! transcript, which holds the turn's messages (issue 260), the prompt
//! parts placed in it - each kept once by digest - and the tools the turn
//! offers, resolved the way a recorded message's context is.

use super::Turn;
use anyhow::{Context, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{
    ContextItem, ContextRef, ContextResolver, FrameRef, KnowledgeRef, PromptPart, PromptRef,
};
use caelumora_model::digest::Digest;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::tool::{Tool, ToolSet};

impl Turn<'_> {
    /// What the model is given for the next step - its messages and the
    /// tools it is offered: the transcript, continued
    /// by the messages the turn produced so far, with the prompt parts where
    /// they were placed - each as the model's profile gives it (issue 63),
    /// every prompt part as a message of the agent in its role - and the
    /// tools the turn offers (issue 137). Resolved from the turn's context the
    /// way a recorded message's context is (`ContextResolver`, issue 136),
    /// so the two cannot diverge. The attachments the model is given along
    /// carry their content (`given_along`).
    ///
    /// # Errors
    /// If the context names what the agent's state does not hold, or an
    /// attachment given along cannot be read.
    pub(crate) async fn full_context(&mut self) -> Result<(Vec<Message>, Vec<Tool>)> {
        let resolved = ContextResolver::of(self.agent_channel_state, self.runtime.prompts())
            .resolve(&self.current().work.context, self.turn_id(), ActorId::Agent(self.agent_id))?
            .context("the context of the turn names what the agent's state does not hold")?;
        let messages = self.given_along(self.as_messages(resolved.items)).await?;
        // The backends are given the tools natively, which knows no sets.
        Ok((messages, resolved.tool_sets.into_iter().flat_map(|set| set.tools).collect()))
    }

    /// The transcript up to and including the entry `last`, on whichever
    /// branch, as the model of the turn would be given it - each message as
    /// its profile gives it, with its time in the turn's time zone (issue
    /// 430), a turn given up told where it ended, one that
    /// no longer flows into the channel reduced to the tools it called -,
    /// without prompt parts or tools (issue 16): what a component hands a
    /// model to work on the conversation itself, a summary say. Resolved as
    /// the turn's own context is (`ContextResolver`), so the same rules hold,
    /// the attachments given along with their content (`given_along`).
    ///
    /// # Errors
    /// If the transcript holds no entry `last`, or an attachment given along
    /// cannot be read.
    pub async fn context_until(&mut self, last: Digest) -> Result<Vec<Message>> {
        let FrameRef { profile, time_zone, .. } = self.current().work.context.frame;
        let reference = ContextRef {
            transcript: Some(last),
            frame: FrameRef { profile, time_zone, ..FrameRef::default() },
        };
        let resolved = ContextResolver::of(self.agent_channel_state, self.runtime.prompts())
            .resolve(&reference, self.turn_id(), ActorId::Agent(self.agent_id))?
            .context("the transcript holds no such entry")?;
        self.given_along(self.as_messages(resolved.items)).await
    }

    /// `messages`, the attachments the turn's model is given along with
    /// their content (issue 282), each seen by the turn as it is given: its
    /// classification counts for what the turn produces from now on, as what
    /// it heard does. One that cannot be read fails the call - a model told
    /// of an image it is not given makes one up.
    async fn given_along(&mut self, mut messages: Vec<Message>) -> Result<Vec<Message>> {
        let profile = self.current().work.context.frame.profile;
        let given = self
            .runtime
            .load_attachments(&mut messages, &profile)
            .await
            .context("an attachment the model is to be given cannot be read")?;
        for classification in &given {
            self.current_mut().work.see(classification);
        }
        Ok(messages)
    }

    /// The model's messages for `items`: a message as it is, a prompt part
    /// as a message of the agent in its role.
    fn as_messages(&self, items: Vec<ContextItem>) -> Vec<Message> {
        items
            .into_iter()
            .map(|item| match item {
                ContextItem::Conversation { message } => message,
                ContextItem::Prompt { role, text, .. } => Message {
                    role,
                    content: Some(text),
                    sender_id: ActorId::Agent(self.agent_id),
                    sender_name: self.agent_name.clone(),
                    ..Message::new(self.current().work.classification.clone())
                },
            })
            .collect()
    }

    /// Offers the turn's model the tools in `tool_sets` - once, at its start,
    /// before its context is set (`AgentChannelState::offer_tools`, issue
    /// 290): what the agent's tool providers offer for the turn's work.
    pub(crate) fn offer_tools(&mut self, tool_sets: Vec<ToolSet>) {
        self.agent_channel_state.offer_tools(tool_sets);
    }

    /// Sets what the `ContextProvider` assembled for the turn - once, at its
    /// start (issue 285): the `knowledge` its prompt parts hold, recorded,
    /// and the `parts` the model is given besides the transcript, where the
    /// provider placed them, all of them seen (`Work::take_in_context`);
    /// what was placed before is replaced. Every part's text is kept once by
    /// digest, so that what the agent saw can be told later (issues 06, 47).
    pub fn set_context(&mut self, parts: Vec<PromptPart>, knowledge: Vec<KnowledgeRef>) {
        self.current_mut().work.take_in_context(&parts, knowledge);
        let prompts = parts
            .into_iter()
            .map(|PromptPart { role, text, after, classification, .. }| {
                self.agent_channel_state.keep_prompt(role, text, after, classification)
            })
            .collect();
        self.current_mut().work.context.frame.prompts = prompts;
    }

    /// Keeps the text of a prompt part once by digest, and returns the
    /// reference to it, given in `role` after the entry `after`, its text
    /// classified `classification`. The model is given it: the turn has
    /// seen it (issue 168).
    pub(super) fn keep_prompt(
        &mut self,
        role: MessageRole,
        text: String,
        after: Option<Digest>,
        classification: Option<Classification>,
    ) -> PromptRef {
        self.current_mut().work.see_classified(classification.as_ref());
        self.agent_channel_state.keep_prompt(role, text, after, classification)
    }

    /// Places `text`, given in `role`, after the messages the turn has
    /// produced so far (issue 49): an instruction of the runtime to the
    /// model, such as the next step of a plan. A prompt part, not a message:
    /// it is kept once by digest like the others, never goes into the
    /// transcript, and is given as the model's profile says a message in
    /// `role` there is to be given. It counts for the messages produced
    /// from now on.
    pub(crate) fn place_turn_prompt(&mut self, role: MessageRole, text: String) {
        let after = self.current().work.context.transcript;
        // Written by the runtime from what the turn produced: as the turn.
        let prompt = self.keep_prompt(role, text, after, Some(self.situation().label()));
        self.current_mut().work.context.frame.prompts.push(prompt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use crate::turn::tests::seen;
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::context::{KnowledgeRef, KnowledgeSource, ModelProfile};
    use caelumora_model::message::MessageId;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::work::Work;
    use caelumora_runtime::transport::Discard;
    use futures::executor;

    /// Issue 282: the model is given along what its profile says - an image, to a model that
    /// sees -, with its content, which the turn has seen then; one whose bytes are not what its
    /// message says fails the call.
    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn the_model_is_given_along_what_its_profile_says_and_the_turn_sees_it() {
        use crate::testing::TestAgents;
        use caelumora_model::channel::testing::bare_channel;
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::config::{AppConfig, BackendConfig, ContentConfig};
        use caelumora_model::content::testing::make_content_ref;
        use caelumora_model::content::{self, ContentId, ContentRef};
        use caelumora_model::digest;
        use caelumora_model::session::testing::bare_session;
        use caelumora_model::session::SessionId;
        use caelumora_runtime::agent::state::AgentStateHandle;
        use caelumora_runtime::session::testing::detached_actor_runtime_in;
        use caelumora_runtime::testing::TestRuntime;
        use std::fs;
        use std::sync::Arc;
        let files = tempfile::tempdir().unwrap();
        let content = ContentConfig {
            storage: [(
                "kept".to_string(),
                BackendConfig::new("local", serde_json::json!({ "root": files.path() })),
            )]
            .into(),
        };
        let runtime = TestRuntime {
            config: AppConfig { content, ..Default::default() },
            plugins: vec![&TestAgents],
            ..Default::default()
        }
        .build();
        let session = bare_session(SessionId::new());
        let channel = bare_channel(session.id);
        let kept = |name: &str, bytes: &[u8], classification: &Classification| {
            let id = ContentId::new();
            let key = format!("attachments/{}/{id}", channel.id);
            fs::create_dir_all(files.path().join(format!("attachments/{}", channel.id))).unwrap();
            fs::write(files.path().join(&key), bytes).unwrap();
            ContentRef {
                id,
                media_type: content::media_type(bytes).to_string(),
                classification: classification.clone(),
                key,
                ..make_content_ref(name, bytes)
            }
        };
        let confidential = Classification::general(Confidentiality::Confidential);
        let png: &[u8] = b"\x89PNG\r\n\x1a\nthe rest";
        let image = kept("chart.png", png, &confidential);
        let notes = kept("notes.txt", b"notes", &Classification::PUBLIC);

        let given = |images: bool, attachments: Vec<ContentRef>| {
            let runtime =
                detached_actor_runtime_in(Arc::clone(&runtime), session.clone(), channel.clone());
            let channel_classification = confidential.clone();
            async move {
                let mut state = AgentChannelState::default();
                state.perceive(vec![Message {
                    role: MessageRole::User,
                    attachments,
                    ..Message::new(Classification::PUBLIC)
                }]);
                let profile = ModelProfile { images, ..ModelProfile::default() };
                state.begin_turn(
                    Work::new(
                        Default::default(),
                        None,
                        Default::default(),
                        channel_classification,
                        Default::default(),
                    )
                    .given_as(profile),
                );
                let mut turn = Turn::of(
                    AgentId::new(),
                    "agent".to_string(),
                    &mut state,
                    AgentStateHandle::detached(),
                    runtime,
                )
                .unwrap();
                let context = turn.full_context().await.map(|(messages, _)| messages);
                (context, turn.situation().label())
            }
        };
        let contents = |messages: &[Message]| {
            messages[0]
                .attachments
                .iter()
                .map(|a| a.content.as_deref().map(<[u8]>::to_vec))
                .collect::<Vec<_>>()
        };

        let (seeing, label) = given(true, vec![image.clone(), notes.clone()]).await;
        assert_eq!(
            contents(&seeing.unwrap()),
            [Some(png.to_vec()), None],
            "the image alone, with its content"
        );
        assert_eq!(label, confidential, "seen as it is given");
        let (blind, label) = given(false, vec![image.clone()]).await;
        assert_eq!(contents(&blind.unwrap()), [None], "nothing to a model that does not see");
        assert_eq!(label, Classification::PUBLIC, "and not seen, though heard");
        let changed = ContentRef { digest: digest::content(b"another"), ..image.clone() };
        let (failed, _) = given(true, vec![changed]).await;
        assert!(failed.is_err(), "not what its message says: the call fails");
    }

    #[tokio::test]
    async fn the_model_is_given_reasoning_as_its_rule_says_and_that_is_recorded() {
        use caelumora_model::context::ReasoningHistory;

        let agent = AgentId::new();
        let thought = |sender: ActorId, content: &str| Message {
            id: MessageId::new(),
            sender_id: sender,
            role: if sender == ActorId::Agent(agent) {
                MessageRole::Assistant
            } else {
                MessageRole::User
            },
            content: Some(content.to_string()),
            reasoning: Some(format!("why {content}")),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let other = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        // An answer of an earlier turn, and another agent's message.
        let earlier = thought(ActorId::Agent(agent), "earlier");
        let theirs = thought(other, "theirs");
        state.transcript.push(earlier.clone());
        state.transcript.push(theirs.clone());

        let profile =
            |reasoning_history| ModelProfile { reasoning_history, ..ModelProfile::default() };
        let first = thought(ActorId::Agent(agent), "first");
        let second = thought(ActorId::Agent(agent), "second");
        let reasoning_of = |messages: &[Message]| {
            messages.iter().map(|m| m.reasoning.is_some()).collect::<Vec<_>>()
        };
        let (first, second) = {
            state.begin_turn(
                Work::new(
                    EventContext::default(),
                    None,
                    SecurityContext::default(),
                    Classification::default(),
                    Default::default(),
                )
                .given_as(profile(ReasoningHistory::CurrentTurn)),
            );
            let mut turn = Turn::under_way(agent, &mut state);
            turn.set_context(Vec::new(), Vec::new());
            let first = turn.push_message(first.clone());
            assert_eq!(
                reasoning_of(&turn.full_context().await.unwrap().0),
                [false, false, true],
                "only the turn's own reasoning"
            );
            let second = turn.push_message(second.clone());
            turn.commit(false, &Discard).await;
            (first, second)
        };

        // What was recorded says what the model was given - not what the
        // transcript holds.
        let reasoning_seen = |state: &AgentChannelState, said: &Message| {
            seen(state, agent, said)
                .expect("the agent produced it")
                .iter()
                .map(|item| match item {
                    ContextItem::Conversation { message } => message.reasoning.is_some(),
                    ContextItem::Prompt { .. } => false,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(reasoning_seen(&state, &first), [false, false]);
        assert_eq!(
            reasoning_seen(&state, &second),
            [false, false, true],
            "the first answer's reasoning, from the same turn"
        );
        assert!(
            state.transcript.messages().iter().all(|m| m.reasoning.is_some()),
            "stored reasoning is untouched"
        );

        // Under `Always`, the agent's own reasoning of earlier turns too -
        // still not another agent's.
        let third = thought(ActorId::Agent(agent), "third");
        state.begin_turn(
            Work::new(
                EventContext::default(),
                None,
                SecurityContext::default(),
                Classification::default(),
                Default::default(),
            )
            .given_as(profile(ReasoningHistory::Always)),
        );
        let mut turn = Turn::under_way(agent, &mut state);
        turn.set_context(Vec::new(), Vec::new());
        turn.push_message(third);
        assert_eq!(
            reasoning_of(&turn.full_context().await.unwrap().0),
            [true, false, true, true, true]
        );
    }

    #[tokio::test]
    async fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        let agent = AgentId::new();
        let text = |role: MessageRole, content: &str| Message {
            id: MessageId::new(),
            role,
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.transcript.push(question.clone());

        let first = text(MessageRole::Assistant, "Let me think.");
        let second = text(MessageRole::Assistant, "4");
        let system = || PromptPart {
            role: MessageRole::System,
            text: "You are good at sums.".to_string(),
            after: None,
            classification: None,
            source: None,
        };
        let order = |items: &[ContextItem]| {
            items
                .iter()
                .map(|item| match item {
                    ContextItem::Prompt { text, role, .. } => format!("{role:?}:{text}"),
                    ContextItem::Conversation { message } => {
                        message.content.clone().unwrap_or_default()
                    }
                })
                .collect::<Vec<_>>()
        };
        let (first, second) = {
            let mut turn = Turn::begun(agent, &mut state);
            turn.set_context(vec![system()], Vec::new());
            let (given, _) = turn.full_context().await.unwrap();
            assert_eq!(given.len(), 2, "the prompt is given to the model as a message");
            assert_eq!(given[0].role, MessageRole::System);
            let first = turn.push_message(first.clone());
            let second = turn.push_message(second.clone());
            turn.commit(false, &Discard).await;
            (first, second)
        };

        assert_eq!(
            order(&seen(&state, agent, &first).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?"],
            "the first answer saw exactly what the turn gave the model"
        );
        assert_eq!(
            order(&seen(&state, agent, &second).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?", "Let me think."],
            "everything before it, its own earlier message included"
        );
        assert!(seen(&state, agent, &question).is_none(), "not produced by the agent");

        // Knowledge read in a turn counts for the messages after it.
        let before = text(MessageRole::Assistant, "Let me look.");
        let after = text(MessageRole::Assistant, "Found it.");
        let read = KnowledgeRef {
            channel: Default::default(),
            session: Default::default(),
            source: KnowledgeSource::History { digest: None, from: None },
            classification: None,
        };
        let (before, after) = {
            let mut turn = Turn::begun(agent, &mut state);
            let before = turn.push_message(before.clone());
            turn.record_knowledge(read.clone());
            turn.record_knowledge(read.clone());
            let after = turn.push_message(after.clone());
            turn.commit(false, &Discard).await;
            (before, after)
        };
        let knowledge_of = |said: &Message| {
            state.context_of(said).expect("sent with what it was produced from").frame.knowledge
        };
        assert!(knowledge_of(&before).is_empty(), "produced before the read");
        assert_eq!(knowledge_of(&after), vec![read], "produced after it, recorded once");

        // The same prompt in another turn is kept once.
        {
            let mut turn = Turn::begun(agent, &mut state);
            turn.set_context(vec![system()], Vec::new());
        }
        assert_eq!(state.prompts.len(), 1);
    }

    #[tokio::test]
    async fn the_tools_a_turn_began_with_are_offered_and_recorded_for_each_message() {
        use caelumora_model::tool::ToolParameters;
        let agent = AgentId::new();
        let tool = |name: &str, requires_approval| Tool {
            name: name.to_string(),
            description: format!("does {name}"),
            parameters: ToolParameters::object(
                Vec::<(String, ToolParameters)>::new(),
                Vec::<String>::new(),
            ),
            requires_approval,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        };
        let answer = |content: &str| Message {
            id: MessageId::new(),
            role: MessageRole::Assistant,
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let mut state = AgentChannelState::default();
        let first = answer("first");
        let second = answer("second");
        let third = answer("third");
        let fourth = answer("fourth");
        let take_turn = |state: &mut AgentChannelState, tools: Vec<Tool>, messages: &[&Message]| {
            let sets = if tools.is_empty() {
                Vec::new()
            } else {
                vec![ToolSet::new("tools", "some tools", tools.clone())]
            };
            state.begin_turn(Work::new(
                Default::default(),
                None,
                Default::default(),
                Default::default(),
                Default::default(),
            ));
            state.offer_tools(sets);
            let mut turn = Turn::under_way(agent, state);
            let (_, offered) = executor::block_on(turn.full_context()).unwrap();
            assert_eq!(offered, tools, "the model is offered what the turn began with");
            let sent: Vec<Message> =
                messages.iter().map(|message| turn.push_message((*message).clone())).collect();
            executor::block_on(turn.commit(false, &Discard));
            sent
        };
        let firsts = take_turn(
            &mut state,
            vec![tool("echo", false), tool("notes", true)],
            &[&first, &second],
        );
        let thirds = take_turn(&mut state, Vec::new(), &[&third]);
        // The same tools, treated otherwise: offered so then.
        let fourths =
            take_turn(&mut state, vec![tool("echo", true), tool("notes", false)], &[&fourth]);

        let catalogue = testing::catalogue();
        let offered = |said: &Message| {
            ContextResolver::of(&state, &catalogue)
                .resolve(
                    &state.context_of(said).expect("sent with it"),
                    said.turn_id.expect("a turn's"),
                    ActorId::Agent(agent),
                )
                .expect("worded")
                .expect("everything it names is here")
                .tool_sets
                .into_iter()
                .flat_map(|set| set.tools)
                .map(|t| (t.name, t.requires_approval))
                .collect::<Vec<_>>()
        };
        let named = |offer: Vec<(String, bool)>| {
            offer
                .into_iter()
                .map(|(name, asks)| format!("{name}{}", if asks { "?" } else { "" }))
                .collect::<Vec<_>>()
        };
        assert_eq!(named(offered(&firsts[0])), ["echo", "notes?"]);
        assert_eq!(
            named(offered(&firsts[1])),
            ["echo", "notes?"],
            "the whole turn offers the same tools"
        );
        assert!(offered(&thirds[0]).is_empty(), "none were offered");
        assert_eq!(
            named(offered(&fourths[0])),
            ["echo?", "notes"],
            "as they were treated when they were offered"
        );
        assert_eq!(state.offered_tools.len(), 2, "each offer kept once");
    }
}
