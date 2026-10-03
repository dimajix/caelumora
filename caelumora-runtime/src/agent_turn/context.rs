//! What the model of a turn is given (issues 47, 49, 63, 136, 137): the
//! transcript continued by the turn's messages, the prompt parts placed in
//! it - each kept once by digest - and the tools the turn offers, resolved
//! the way a recorded message's context is.

use super::Turn;
use anyhow::{Context, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{ContextItem, ContextResolver, PromptPart, PromptRef};
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::tool::ToolDefinition;

impl Turn<'_> {
    /// What the model is given for the next step - its messages and the
    /// tools it is offered: the transcript, continued
    /// by the messages the turn produced so far, with the prompt parts where
    /// they were placed - each as the model's profile gives it (issue 63),
    /// every prompt part as a message of the agent in its role - and the
    /// tools the turn offers (issue 137). Resolved from the turn's context the
    /// way a recorded message's context is (`ContextResolver`, issue 136),
    /// so the two cannot diverge.
    pub(crate) fn full_context(&self) -> Result<(Vec<Message>, Vec<ToolDefinition>)> {
        let resolved = ContextResolver::of(self.agent_channel_state)
            .resolve(&self.current().work.context, ActorId::Agent(self.agent_id))
            .context("the context of the turn names what the agent's state does not hold")?;
        let messages = resolved.items.into_iter()
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
            .collect();
        Ok((messages, resolved.tools))
    }

    /// Sets the prompt parts the model of the turn is given besides the
    /// transcript, where the `ContextProvider` placed them - once, at the
    /// start of the turn; what was placed before is replaced. Every part's
    /// text is kept once by digest, so that what the agent saw can be told
    /// later (issues 06, 47).
    pub fn set_context(&mut self, parts: Vec<PromptPart>) {
        let prompts = parts.into_iter()
            .map(|PromptPart { role, text, after, classification }| self.keep_prompt(role, text, after, classification))
            .collect();
        self.current_mut().work.context.prompts = prompts;
    }

    /// Keeps the text of a prompt part once by digest, and returns the
    /// reference to it, given in `role` after the entry `after`, its text
    /// classified `classification`. The model is given it: the turn has
    /// seen it (issue 168).
    fn keep_prompt(&mut self, role: MessageRole, text: String, after: Option<caelumora_model::digest::Digest>, classification: Option<Classification>) -> PromptRef {
        self.current_mut().work.see_classified(classification.as_ref());
        let digest = caelumora_model::digest::prompt(&text);
        self.agent_channel_state.prompts.keep(digest, || text);
        PromptRef { text: digest, role, after, classification }
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
        let prompt = self.keep_prompt(role, text, after, Some(self.label()));
        self.current_mut().work.context.prompts.push(prompt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_turn::tests::seen;
    use caelumora_model::actor::EventContext;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::context::{KnowledgeRef, ModelProfile};
    use caelumora_model::message::MessageId;
    use caelumora_model::tool::Tool;

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
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
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
        {
            state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::default(), Vec::new(), profile(ReasoningHistory::CurrentTurn));
            let mut turn = Turn::of(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached()).expect("a turn was begun");
            turn.set_context(Vec::new());
            turn.push_message(first.clone());
            assert_eq!(reasoning_of(&turn.full_context().unwrap().0), [false, false, true], "only the turn's own reasoning");
            turn.push_message(second.clone());
            turn.commit(false).await;
        }

        // What was recorded says what the model was given - not what the
        // transcript holds.
        let reasoning_seen = |state: &AgentChannelState, id| seen(state, agent, id).expect("the agent produced it").iter().map(|item| match item {
            ContextItem::Conversation { message } => message.reasoning.is_some(),
            ContextItem::Prompt { .. } => false,
        }).collect::<Vec<_>>();
        assert_eq!(reasoning_seen(&state, first.id), [false, false]);
        assert_eq!(reasoning_seen(&state, second.id), [false, false, true], "the first answer's reasoning, from the same turn");
        assert!(state.transcript.messages().iter().all(|m| m.reasoning.is_some()), "stored reasoning is untouched");

        // Under `Always`, the agent's own reasoning of earlier turns too -
        // still not another agent's.
        let third = thought(ActorId::Agent(agent), "third");
        state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::default(), Vec::new(), profile(ReasoningHistory::Always));
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached()).expect("a turn was begun");
        turn.set_context(Vec::new());
        turn.push_message(third);
        assert_eq!(reasoning_of(&turn.full_context().unwrap().0), [true, false, true, true, true]);
    }

    #[tokio::test]
    async fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        let agent = AgentId::new();
        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), role, content: Some(content.to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.transcript.push(question.clone());

        let (first, second) = (text(MessageRole::Assistant, "Let me think."), text(MessageRole::Assistant, "4"));
        let system = || PromptPart { role: MessageRole::System, text: "You are good at sums.".to_string(), after: None, classification: None };
        let order = |items: &[ContextItem]| items.iter().map(|item| match item {
            ContextItem::Prompt { text, role, .. } => format!("{role:?}:{text}"),
            ContextItem::Conversation { message } => message.content.clone().unwrap_or_default(),
        }).collect::<Vec<_>>();
        {
            let mut turn = Turn::begun(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.set_context(vec![system()]);
            let (given, _) = turn.full_context().unwrap();
            assert_eq!(given.len(), 2, "the prompt is given to the model as a message");
            assert_eq!(given[0].role, MessageRole::System);
            turn.push_message(first.clone());
            turn.push_message(second.clone());
            turn.commit(false).await;
        }

        assert_eq!(order(&seen(&state, agent, first.id).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?"], "the first answer saw exactly what the turn gave the model");
        assert_eq!(order(&seen(&state, agent, second.id).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?", "Let me think."], "everything before it, its own earlier message included");
        assert!(state.recorded_context_of(question.id).is_none(), "not produced by the agent");

        // Knowledge read in a turn counts for the messages after it.
        let (before, after) = (text(MessageRole::Assistant, "Let me look."), text(MessageRole::Assistant, "Found it."));
        let read = KnowledgeRef {
            channel: Default::default(),
            session: Default::default(),
            source: caelumora_model::context::KnowledgeSource::History { digest: None, start: None },
            classification: None,
        };
        {
            let mut turn = Turn::begun(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.push_message(before.clone());
            turn.record_knowledge(read.clone());
            turn.record_knowledge(read.clone());
            turn.push_message(after.clone());
            turn.commit(false).await;
        }
        let knowledge_of = |id| state.recorded_context_of(id).unwrap().knowledge;
        assert!(knowledge_of(before.id).is_empty(), "produced before the read");
        assert_eq!(knowledge_of(after.id), vec![read], "produced after it, recorded once");

        // The same prompt in another turn is kept once.
        {
            let mut turn = Turn::begun(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.set_context(vec![system()]);
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
        };
        let answer = |content: &str| Message { id: MessageId::new(), role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        let (first, second, third, fourth) = (answer("first"), answer("second"), answer("third"), answer("fourth"));
        let take_turn = |state: &mut AgentChannelState, tools: Vec<Tool>, messages: &[&Message]| {
            state.begin_turn(Default::default(), Default::default(), Default::default(), tools.clone(), Default::default());
            let mut turn = Turn::of(agent, "agent".to_string(), state, crate::agent_state::AgentStateHandle::detached()).expect("a turn was begun");
            let (_, offered) = turn.full_context().unwrap();
            assert_eq!(offered, tools.iter().map(Tool::definition).collect::<Vec<_>>(), "the model is offered what the turn began with");
            for message in messages {
                turn.push_message((*message).clone());
            }
            futures::executor::block_on(turn.commit(false));
        };
        take_turn(&mut state, vec![tool("echo", false), tool("notes", true)], &[&first, &second]);
        take_turn(&mut state, Vec::new(), &[&third]);
        // Only what the model is told of a tool counts for its set.
        take_turn(&mut state, vec![tool("echo", true), tool("notes", false)], &[&fourth]);

        let offered = |id| ContextResolver::of(&state).resolve(&state.recorded_context_of(id).unwrap(), ActorId::Agent(agent))
            .expect("everything it names is here").tools.into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(offered(first.id), ["echo", "notes"]);
        assert_eq!(offered(second.id), ["echo", "notes"], "the whole turn offers the same tools");
        assert!(offered(third.id).is_empty(), "none were offered");
        assert_eq!(offered(fourth.id), ["echo", "notes"]);
        assert_eq!(state.tool_sets.len(), 1, "a set offered again is kept once");
    }
}
