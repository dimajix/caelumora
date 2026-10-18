//! What the model of a turn is given (issues 47, 49, 63, 136, 137): the
//! transcript, which holds the turn's messages (issue 260), the prompt
//! parts placed in it - each kept once by digest - and the tools the turn
//! offers, resolved the way a recorded message's context is.

use super::Turn;
use anyhow::{Context, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{ContextItem, ContextRef, ContextResolver, KnowledgeRef, PromptPart, PromptRef};
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
    /// so the two cannot diverge.
    pub(crate) fn full_context(&self) -> Result<(Vec<Message>, Vec<Tool>)> {
        let resolved = ContextResolver::of(self.agent_channel_state)
            .resolve(&self.current().work.context, self.turn_id(), ActorId::Agent(self.agent_id))
            .context("the context of the turn names what the agent's state does not hold")?;
        let messages = self.as_messages(resolved.items);
        // The backends are given the tools natively, which knows no sets.
        Ok((messages, resolved.tool_sets.into_iter().flat_map(|set| set.tools).collect()))
    }

    /// The transcript up to and including the entry `last`, on whichever
    /// branch, as the model of the turn would be given it - each message as
    /// its profile gives it, a turn given up told where it ended, one that
    /// no longer flows into the channel reduced to the tools it called -,
    /// without prompt parts or tools (issue 16): what a component hands a
    /// model to work on the conversation itself, a summary say. Resolved as
    /// the turn's own context is (`ContextResolver`), so the same rules hold.
    ///
    /// # Errors
    /// If the transcript holds no entry `last`.
    pub fn context_until(&self, last: Digest) -> Result<Vec<Message>> {
        let reference = ContextRef { transcript: Some(last), profile: self.current().work.context.profile, ..ContextRef::default() };
        let resolved = ContextResolver::of(self.agent_channel_state)
            .resolve(&reference, self.turn_id(), ActorId::Agent(self.agent_id))
            .context("the transcript holds no such entry")?;
        Ok(self.as_messages(resolved.items))
    }

    /// The model's messages for `items`: a message as it is, a prompt part
    /// as a message of the agent in its role.
    fn as_messages(&self, items: Vec<ContextItem>) -> Vec<Message> {
        items.into_iter()
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
        let prompts = parts.into_iter()
            .map(|PromptPart { role, text, after, classification, .. }| self.agent_channel_state.keep_prompt(role, text, after, classification))
            .collect();
        self.current_mut().work.context.prompts = prompts;
    }

    /// Keeps the text of a prompt part once by digest, and returns the
    /// reference to it, given in `role` after the entry `after`, its text
    /// classified `classification`. The model is given it: the turn has
    /// seen it (issue 168).
    pub(super) fn keep_prompt(&mut self, role: MessageRole, text: String, after: Option<Digest>, classification: Option<Classification>) -> PromptRef {
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
        self.current_mut().work.context.prompts.push(prompt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_runtime::transport::Discard;
    use crate::turn::tests::seen;
    use caelumora_model::actor::EventContext;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::context::{KnowledgeRef, ModelProfile, KnowledgeSource};
    use caelumora_model::message::MessageId;
    use caelumora_model::tool::{Tool, ToolSet};

    #[tokio::test]
    async fn the_model_is_given_reasoning_as_its_rule_says_and_that_is_recorded() {
        use caelumora_model::context::ReasoningHistory;

        let agent = AgentId::new();
        let thought = |sender: ActorId, content: &str| Message {
            id: MessageId::new(),
            sender_id: sender,
            role: if sender == ActorId::Agent(agent) { MessageRole::Assistant } else { MessageRole::User },
            content: Some(content.to_string()),
            reasoning: Some(format!("why {content}")),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let other = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        // An answer of an earlier turn, and another agent's message.
        let (earlier, theirs) = (thought(ActorId::Agent(agent), "earlier"), thought(other, "theirs"));
        state.transcript.push(earlier.clone());
        state.transcript.push(theirs.clone());

        let profile = |reasoning_history| ModelProfile { reasoning_history, ..ModelProfile::default() };
        let (first, second) = (thought(ActorId::Agent(agent), "first"), thought(ActorId::Agent(agent), "second"));
        let reasoning_of = |messages: &[Message]| messages.iter().map(|m| m.reasoning.is_some()).collect::<Vec<_>>();
        let (first, second) = {
            state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::default(), profile(ReasoningHistory::CurrentTurn), Default::default());
            let mut turn = Turn::under_way(agent, &mut state);
            turn.set_context(Vec::new(), Vec::new());
            let first = turn.push_message(first.clone());
            assert_eq!(reasoning_of(&turn.full_context().unwrap().0), [false, false, true], "only the turn's own reasoning");
            let second = turn.push_message(second.clone());
            turn.commit(false, &Discard).await;
            (first, second)
        };

        // What was recorded says what the model was given - not what the
        // transcript holds.
        let reasoning_seen = |state: &AgentChannelState, said: &Message| seen(state, agent, said).expect("the agent produced it").iter().map(|item| match item {
            ContextItem::Conversation { message } => message.reasoning.is_some(),
            ContextItem::Prompt { .. } => false,
        }).collect::<Vec<_>>();
        assert_eq!(reasoning_seen(&state, &first), [false, false]);
        assert_eq!(reasoning_seen(&state, &second), [false, false, true], "the first answer's reasoning, from the same turn");
        assert!(state.transcript.messages().iter().all(|m| m.reasoning.is_some()), "stored reasoning is untouched");

        // Under `Always`, the agent's own reasoning of earlier turns too -
        // still not another agent's.
        let third = thought(ActorId::Agent(agent), "third");
        state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::default(), profile(ReasoningHistory::Always), Default::default());
        let mut turn = Turn::under_way(agent, &mut state);
        turn.set_context(Vec::new(), Vec::new());
        turn.push_message(third);
        assert_eq!(reasoning_of(&turn.full_context().unwrap().0), [true, false, true, true, true]);
    }

    #[tokio::test]
    async fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        let agent = AgentId::new();
        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), role, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.transcript.push(question.clone());

        let (first, second) = (text(MessageRole::Assistant, "Let me think."), text(MessageRole::Assistant, "4"));
        let system = || PromptPart { role: MessageRole::System, text: "You are good at sums.".to_string(), after: None, classification: None, source: None };
        let order = |items: &[ContextItem]| items.iter().map(|item| match item {
            ContextItem::Prompt { text, role, .. } => format!("{role:?}:{text}"),
            ContextItem::Conversation { message } => message.content.clone().unwrap_or_default(),
        }).collect::<Vec<_>>();
        let (first, second) = {
            let mut turn = Turn::begun(agent, &mut state);
            turn.set_context(vec![system()], Vec::new());
            let (given, _) = turn.full_context().unwrap();
            assert_eq!(given.len(), 2, "the prompt is given to the model as a message");
            assert_eq!(given[0].role, MessageRole::System);
            let first = turn.push_message(first.clone());
            let second = turn.push_message(second.clone());
            turn.commit(false, &Discard).await;
            (first, second)
        };

        assert_eq!(order(&seen(&state, agent, &first).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?"], "the first answer saw exactly what the turn gave the model");
        assert_eq!(order(&seen(&state, agent, &second).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?", "Let me think."], "everything before it, its own earlier message included");
        assert!(seen(&state, agent, &question).is_none(), "not produced by the agent");

        // Knowledge read in a turn counts for the messages after it.
        let (before, after) = (text(MessageRole::Assistant, "Let me look."), text(MessageRole::Assistant, "Found it."));
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
        let knowledge_of = |said: &Message| said.produced_from.clone().expect("sent with what it was produced from").knowledge;
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
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        };
        let answer = |content: &str| Message { id: MessageId::new(), role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        let (first, second, third, fourth) = (answer("first"), answer("second"), answer("third"), answer("fourth"));
        let take_turn = |state: &mut AgentChannelState, tools: Vec<Tool>, messages: &[&Message]| {
            let sets = if tools.is_empty() { Vec::new() } else { vec![ToolSet::new("tools", "some tools", tools.clone())] };
            state.begin_turn(Default::default(), Default::default(), Default::default(), Default::default(), Default::default());
            state.offer_tools(sets);
            let mut turn = Turn::under_way(agent, state);
            let (_, offered) = turn.full_context().unwrap();
            assert_eq!(offered, tools, "the model is offered what the turn began with");
            let sent: Vec<Message> = messages.iter().map(|message| turn.push_message((*message).clone())).collect();
            futures::executor::block_on(turn.commit(false, &Discard));
            sent
        };
        let firsts = take_turn(&mut state, vec![tool("echo", false), tool("notes", true)], &[&first, &second]);
        let thirds = take_turn(&mut state, Vec::new(), &[&third]);
        // The same tools, treated otherwise: offered so then.
        let fourths = take_turn(&mut state, vec![tool("echo", true), tool("notes", false)], &[&fourth]);

        let offered = |said: &Message| ContextResolver::of(&state).resolve(said.produced_from.as_ref().expect("sent with it"), said.turn_id.expect("a turn's"), ActorId::Agent(agent))
            .expect("everything it names is here").tool_sets.into_iter().flat_map(|set| set.tools).map(|t| (t.name, t.requires_approval)).collect::<Vec<_>>();
        let named = |offer: Vec<(String, bool)>| offer.into_iter().map(|(name, asks)| format!("{name}{}", if asks { "?" } else { "" })).collect::<Vec<_>>();
        assert_eq!(named(offered(&firsts[0])), ["echo", "notes?"]);
        assert_eq!(named(offered(&firsts[1])), ["echo", "notes?"], "the whole turn offers the same tools");
        assert!(offered(&thirds[0]).is_empty(), "none were offered");
        assert_eq!(named(offered(&fourths[0])), ["echo?", "notes"], "as they were treated when they were offered");
        assert_eq!(state.offered_tools.len(), 2, "each offer kept once");
    }
}
