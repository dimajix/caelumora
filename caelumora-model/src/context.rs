//! What an agent's model is given, and what is recorded of it (issues 06,
//! 47, 49, 63; see `docs/AGENT-TURN.md`).
//!
//! What is stored are facts: the agent's `Transcript`, and the text of every
//! prompt part and every set of tools offered once by digest. What the model is given is not stored but
//! resolved from them:
//!
//! - A turn holds a [`ContextRef`] for its next message: the transcript so
//!   far, the prompt parts placed in it, the tools offered, the knowledge
//!   read and the rules of the agent's model ([`ModelProfile`]). With every message it produces,
//!   the reference as it stood for that message is kept and, when the turn
//!   is taken over, recorded in the history.
//! - The messages of a turn under way are not in the transcript yet, but
//!   have the digests they will have there: a [`TranscriptView`] continues
//!   the stored transcript with them (issue 136).
//! - The [`ContextResolver`] resolves a reference against that view and the
//!   prompt texts - for the next model call as for a message produced
//!   earlier. What the model was given and what is told as given therefore
//!   cannot diverge.

use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::agent::AgentChannelState;
use crate::turn::TurnEntry;
use crate::chain::Node;
use crate::digest::Digest;
use crate::kept::Kept;
use crate::message::{Message, MessageRole};
use crate::transcript::{Transcript, TranscriptEntry};
use crate::tool::ToolDefinition;
use serde::{Deserialize, Serialize};

/// A prompt part an agent's model was given besides the conversation - the
/// system prompt, notes, ... (issue 47): the digest of its text, the role it
/// was given as, and the entry of the transcript it followed, `None` if it
/// came before everything - and how what it holds besides the knowledge it
/// lists was classified (issue 168), `None` if that is not known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRef {
    pub text: Digest,
    pub role: MessageRole,
    pub after: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
}

/// An element of what an agent's model is given, in the shape it was given
/// (issue 119): a message of the conversation, or a prompt part where the
/// `ContextProvider` placed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextItem {
    /// A message of the conversation, as the model was given it.
    Conversation { message: Message },
    /// A prompt part - the system prompt, notes, ... - that is not part of
    /// the conversation, given to the model in `role`; `digest` is the
    /// digest of `text`, by which the part is kept once and referred to;
    /// `classification` how it was classified, `None` if that is not known
    /// (`PromptRef::classification`).
    Prompt {
        digest: Digest,
        role: MessageRole,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        classification: Option<Classification>,
    },
}

impl ContextItem {
    /// The role the item is given in.
    pub fn role(&self) -> MessageRole {
        match self {
            ContextItem::Conversation { message } => message.role,
            ContextItem::Prompt { role, .. } => *role,
        }
    }
}

/// Knowledge from another place an agent's model was given (issue 17): what
/// was read, from which channel - of which session (issue 134) - as the
/// digest of the state read, and how what was read was classified when it
/// was read (issue 168): the join of its entries, `None` if that is not
/// known - read before it was recorded, or entries without an audience in
/// common.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRef {
    pub channel: crate::channel::ChannelId,
    /// The session of `channel`: whether it was read beyond the agent's own.
    pub session: crate::session::SessionId,
    pub source: KnowledgeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
}

/// What a [`KnowledgeRef`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnowledgeSource {
    /// The notes an agent published in the channel; the digest of its note
    /// chain, `None` while it had none.
    Notes { agent: crate::agent::AgentId, digest: Option<Digest> },
    /// An agent's private notes in the channel (issue 51), read by that
    /// agent; the digest of its private note chain.
    PrivateNotes { agent: crate::agent::AgentId, digest: Option<Digest> },
    /// The channel's history; the digest of it when it was read - up to
    /// where - and, for a reader who joined later with a limit, where their
    /// view began (issue 72). `None`: all of it.
    History {
        digest: Option<Digest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<crate::channel::HistoryStart>,
    },
    /// An agent's memory (issue 62), read by that agent; the digest of its
    /// memory chain. Recorded once for every channel the entries recalled
    /// were written in, so that knowledge from another session can be told
    /// (issue 134).
    Memory { agent: crate::agent::AgentId, digest: Option<Digest> },
}

/// Which reasoning of the messages in its context a model is given back
/// (issue 63). Some models must not see earlier reasoning again, others
/// need the reasoning that led to a tool call within the same turn. Stored
/// reasoning is not touched: this decides only what a model call is given.
///
/// Whatever the rule, a model is never given another actor's reasoning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningHistory {
    /// No reasoning at all, not even of the turn under way.
    Never,
    /// The agent's own reasoning of the turn under way, not that of earlier
    /// turns - what reasoning models document. The default.
    #[default]
    CurrentTurn,
    /// The agent's own reasoning of every turn.
    Always,
}

impl ReasoningHistory {
    /// `message` as the model of `agent` is given it: without its reasoning
    /// unless the rule keeps it. `in_turn` says whether the turn under way
    /// produced the message.
    pub fn given(self, mut message: Message, agent: ActorId, in_turn: bool) -> Message {
        let keeps = message.sender_id == agent && match self {
            Self::Never => false,
            Self::CurrentTurn => in_turn,
            Self::Always => true,
        };
        if !keeps {
            message.reasoning = None;
        }
        message
    }
}

/// How a model is to be given its context: a property of the model,
/// declared by the agent's `LLMClient` and applied where the model's input
/// is resolved (`ContextResolver`), so that a backend gets only what its
/// model is to see. A closed set of rules, as data, so that it can be
/// recorded with every message and applied again when what the model saw
/// is asked for (issue 16's history).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Which reasoning of earlier messages the model is given back.
    pub reasoning_history: ReasoningHistory,
    /// Where the model keeps system messages.
    pub system_messages: SystemMessages,
}

impl ModelProfile {
    /// `message` as the model of `agent` is given it; `in_turn` says whether
    /// the turn under way produced the message.
    pub fn given(&self, message: Message, agent: ActorId, in_turn: bool) -> Message {
        self.reasoning_history.given(message, agent, in_turn)
    }

    /// Gives every system message of `items` that does not lead them - a
    /// prompt part or a message of the conversation - in the role the
    /// model keeps where it stands (issue 49).
    fn place_system_messages(&self, items: &mut [ContextItem]) {
        if self.system_messages == SystemMessages::Anywhere {
            return;
        }
        let leading = items.iter().take_while(|item| item.role() == MessageRole::System).count();
        for item in &mut items[leading..] {
            let role = match item {
                ContextItem::Conversation { message } => &mut message.role,
                ContextItem::Prompt { role, .. } => role,
            };
            if *role == MessageRole::System {
                *role = MessageRole::User;
            }
        }
    }
}

/// Where a model keeps system messages (issue 49). Some models' chat
/// templates merge every system message into the system prompt and move it
/// to the front, so that a system message in the middle of the context
/// loses its place - and, changing the system prompt, the prompt cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessages {
    /// Only those before everything else, the system prompt; any later one
    /// is given as a user message. The default: it holds for every model.
    #[default]
    OnlyAtStart,
    /// Wherever they stand.
    Anywhere,
}

/// A prompt part as a `ContextProvider` places it: its text, the role it is
/// given in, and the transcript entry it follows - `None`: before
/// everything - and how what it holds besides knowledge is classified,
/// `None` if that is not known (issue 168).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPart {
    pub role: MessageRole,
    pub text: String,
    pub after: Option<Digest>,
    pub classification: Option<Classification>,
}

/// What an agent's model saw when it produced a message (issue 06): the
/// digest of its `Transcript` before the message - `None` if it was
/// empty - the prompt parts of that turn, each where it was placed (issue
/// 47), the tools it was offered (issue 137), the knowledge from other
/// places read before the message was produced (issue 17), and the rules of
/// the model (issue 63). A turn keeps one for its next message
/// (`Work::context` of its `TurnState`); resolved by the
/// [`ContextResolver`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRef {
    /// The digest of the agent's transcript before the message.
    pub transcript: Option<Digest>,
    pub prompts: Vec<PromptRef>,
    /// The digest of the set of tools the model was offered
    /// (`crate::digest::tools`), `None` if it was offered none.
    pub tools: Option<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<KnowledgeRef>,
    /// The rules the model was given its context by.
    pub profile: ModelProfile,
    /// How many of the transcript's last entries the turn that produced the
    /// message had produced before it: the ones given as the turn's own.
    pub turn_entries: usize,
}

/// A context resolved (issues 06, 47): what the model of an agent was given
/// for a message, in the order it was given it, and the tools it was offered
/// (issue 137), with the reference it was resolved from (issue 119).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// What the history recorded for the message.
    pub reference: ContextRef,
    pub items: Vec<ContextItem>,
    pub tools: Vec<ToolDefinition>,
}

/// An agent's transcript continued by the messages of its turn under way
/// (issue 136), which have the digests they will have once the turn is
/// taken over. Only the stored part is stored; the view is what references
/// are resolved against while a turn runs.
#[derive(Debug, Clone, Copy)]
pub struct TranscriptView<'a> {
    stored: &'a Transcript,
    pending: &'a [TurnEntry],
}

impl<'a> TranscriptView<'a> {
    /// The transcript of `state`, continued by its turn under way, if any.
    pub fn of(state: &'a AgentChannelState) -> Self {
        let pending = state.turn.as_ref().map(|turn| turn.entries.as_slice()).unwrap_or_default();
        Self { stored: &state.transcript, pending }
    }

    /// The entries of the transcript `digest` names - `None`: the empty
    /// one - or `None` if it is not a prefix of this one.
    pub fn prefix(&self, digest: Option<&Digest>) -> Option<Vec<&'a Node<TranscriptEntry>>> {
        let Some(digest) = digest else { return Some(Vec::new()) };
        if let Some(entries) = self.stored.prefix(digest) {
            return Some(entries);
        }
        let end = self.pending.iter().position(|e| &e.node.digest == digest)?;
        Some(self.stored.entries().chain(self.pending[..=end].iter().map(|e| &e.node)).collect())
    }
}

/// Resolves references against an agent's data in a channel - its
/// transcript, continued by its turn under way, the texts of its prompt
/// parts and the sets of tools it was offered - into what its model is
/// given. One implementation for the model
/// call and for telling afterwards what the model saw.
#[derive(Debug, Clone, Copy)]
pub struct ContextResolver<'a> {
    transcript: TranscriptView<'a>,
    prompts: &'a Kept<String>,
    tool_sets: &'a Kept<Vec<ToolDefinition>>,
}

impl<'a> ContextResolver<'a> {
    /// The resolver over the data of `state`.
    pub fn of(state: &'a AgentChannelState) -> Self {
        Self { transcript: TranscriptView::of(state), prompts: &state.prompts, tool_sets: &state.tool_sets }
    }

    /// What the model of `agent` is given, or was given, for the message
    /// `reference` is for - the next one of the turn under way (`Work::context`
    /// of its `TurnState`) or one recorded in the history: the transcript it
    /// names, of which the last `turn_entries` are the turn's
    /// own, with the prompt parts where they were placed, and the tools it
    /// was offered. `None` if what it names is not here - a transcript that
    /// is not a prefix of this one, a prompt or a set of tools that was
    /// never recorded, or a prompt whose entry is not in the prefix.
    pub fn resolve(&self, reference: &ContextRef, agent: ActorId) -> Option<ResolvedContext> {
        let entries = self.transcript.prefix(reference.transcript.as_ref())?;
        let turn_start = entries.len().checked_sub(reference.turn_entries)?;
        let (before, during) = entries.split_at(turn_start);
        let items = self.assemble(&reference.prompts, &reference.profile, agent, before, during)?;
        let tools = match &reference.tools {
            Some(digest) => self.tool_sets.get(digest)?.clone(),
            None => Vec::new(),
        };
        Some(ResolvedContext { reference: reference.clone(), items, tools })
    }

    /// The items the model of `agent` is given: the entries `before` the
    /// turn, then those `during` it, as `profile` gives them, with every
    /// part of `prompts` after the entry it follows. `None` if a part's text
    /// is not kept or its entry is not there.
    fn assemble(
        &self,
        prompts: &[PromptRef],
        profile: &ModelProfile,
        agent: ActorId,
        before: &[&Node<TranscriptEntry>],
        during: &[&Node<TranscriptEntry>],
    ) -> Option<Vec<ContextItem>> {
        // The parts after the transcript entry `after` - `None`: before
        // everything.
        let after_entry = |after: Option<Digest>| -> Option<Vec<ContextItem>> {
            prompts.iter()
                .filter(|p| p.after == after)
                .map(|p| Some(ContextItem::Prompt { digest: p.text, role: p.role, text: self.prompts.get(&p.text)?.clone(), classification: p.classification.clone() }))
                .collect()
        };

        let mut items = after_entry(None)?;
        let entries = before.iter().map(|e| (e, false)).chain(during.iter().map(|e| (e, true)));
        for (node, in_turn) in entries {
            // The record of a turn given up is no message; a part may still
            // have been placed after it.
            if let Some(message) = node.entry.message() {
                items.push(ContextItem::Conversation { message: profile.given(message.clone(), agent, in_turn) });
            }
            items.extend(after_entry(Some(node.digest))?);
        }
        profile.place_system_messages(&mut items);
        // Every part was placed, none after what is not there.
        let placed = items.iter().filter(|item| matches!(item, ContextItem::Prompt { .. })).count();
        (placed == prompts.len()).then_some(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;
    use crate::turn::TurnState;
    use crate::digest::prompt;
    use crate::message::MessageId;

    fn text(content: &str) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::new(crate::confidentiality::Classification::CHANNEL_DEFAULT) }
    }

    fn contents(items: &[ContextItem]) -> Vec<String> {
        items.iter().map(|item| match item {
            ContextItem::Prompt { text, .. } => format!("prompt:{text}"),
            ContextItem::Conversation { message } => message.content.clone().unwrap_or_default(),
        }).collect()
    }

    #[test]
    fn reasoning_is_given_back_as_the_models_rule_says() {
        let agent = ActorId::Agent(AgentId::new());
        let other = ActorId::Agent(AgentId::new());
        let thought = |sender: ActorId| Message { sender_id: sender, reasoning: Some("thinking".to_string()), ..text("hello") };
        let kept = |rule: ReasoningHistory, sender: ActorId, in_turn: bool| rule.given(thought(sender), agent, in_turn).reasoning.is_some();

        assert!(!kept(ReasoningHistory::Never, agent, true), "never: not even the turn's own");
        assert!(kept(ReasoningHistory::CurrentTurn, agent, true), "the turn under way");
        assert!(!kept(ReasoningHistory::CurrentTurn, agent, false), "not of earlier turns");
        assert!(kept(ReasoningHistory::Always, agent, false), "always: every turn's");
        for rule in [ReasoningHistory::Never, ReasoningHistory::CurrentTurn, ReasoningHistory::Always] {
            assert!(!kept(rule, other, true), "{rule:?}: never another actor's");
        }
        assert_eq!(ReasoningHistory::given(ReasoningHistory::Never, thought(agent), agent, true).content, thought(agent).content, "only the reasoning goes");
        assert_eq!(ReasoningHistory::default(), ReasoningHistory::CurrentTurn);
    }

    #[test]
    fn prompt_parts_are_resolved_where_they_were_placed() {
        let mut state = AgentChannelState::default();
        let (earlier, question) = (text("earlier"), text("question"));
        let after_earlier = state.transcript.push(earlier.clone());
        assert_eq!(after_earlier, None);
        let before_question = state.transcript.push(question.clone());
        let whole = state.transcript.digest();
        for part in ["system", "notes"] {
            state.prompts.keep(prompt(part), || part.to_string());
        }
        let reference = ContextRef {
            transcript: whole,
            prompts: vec![
                PromptRef { text: prompt("system"), role: MessageRole::System, after: None, classification: None },
                PromptRef { text: prompt("notes"), role: MessageRole::User, after: before_question, classification: None },
            ],
            tools: None,
            knowledge: Vec::new(),
            profile: ModelProfile::default(),
            turn_entries: 0,
        };

        let resolver = ContextResolver::of(&state);
        let seen = resolver.resolve(&reference, ActorId::None).expect("everything it names is here");
        assert_eq!(contents(&seen.items), ["prompt:system", "earlier", "prompt:notes", "question"]);

        let mut elsewhere = reference.clone();
        elsewhere.prompts[1].after = Some(prompt("not an entry"));
        assert!(resolver.resolve(&elsewhere, ActorId::None).is_none(), "a part after an entry that is not in the prefix");
        let mut forgotten = reference;
        forgotten.prompts[0].text = prompt("never recorded");
        assert!(resolver.resolve(&forgotten, ActorId::None).is_none(), "a part whose text was not kept");
    }

    /// A turn of `state` begun on its transcript as it stands.
    fn begin(state: &mut AgentChannelState, profile: ModelProfile) {
        let context = ContextRef { transcript: state.transcript.digest(), profile, ..ContextRef::default() };
        state.turn = Some(TurnState { work: crate::work::Work { context, ..Default::default() }, ..TurnState::default() });
    }

    /// A prompt part `text` in `role` after the end of `state`'s turn so
    /// far, its text kept.
    fn place(state: &mut AgentChannelState, role: MessageRole, text: &str) {
        state.prompts.keep(prompt(text), || text.to_string());
        let turn = state.turn.as_mut().unwrap();
        turn.work.context.prompts.push(PromptRef { text: prompt(text), role, after: turn.work.context.transcript, classification: None });
    }

    #[test]
    fn a_turn_under_way_is_resolved_as_its_messages_are_recorded() {
        let agent = ActorId::Agent(AgentId::new());
        let own = |content: &str| Message { sender_id: agent, reasoning: Some(format!("why {content}")), ..text(content) };
        let mut state = AgentChannelState::default();
        state.transcript.push(own("earlier"));
        state.transcript.push(text("question"));

        begin(&mut state, ModelProfile::default());
        // Instructions of the runtime among the turn's messages: one before
        // them, one after the first.
        place(&mut state, MessageRole::User, "step 1");
        state.turn.as_mut().unwrap().push(own("first"));
        place(&mut state, MessageRole::User, "step 2");
        state.turn.as_mut().unwrap().push(own("second"));

        let next = state.turn.as_ref().unwrap().work.context.clone();
        let current = ContextResolver::of(&state).resolve(&next, agent).expect("the view holds the turn's messages");
        assert_eq!(contents(&current.items), ["earlier", "question", "prompt:step 1", "first", "prompt:step 2", "second"]);
        let reasoning = current.items.iter().map(|item| matches!(item, ContextItem::Conversation { message } if message.reasoning.is_some())).collect::<Vec<_>>();
        assert_eq!(reasoning, [false, false, false, true, false, true], "the turn's own reasoning, not an earlier turn's");

        // What each message was produced from, kept with it, resolves - now
        // and once the turn is in the transcript - to what the model was
        // given for it.
        let turn = state.turn.take().unwrap();
        let seen = |state: &AgentChannelState, index: usize| contents(&ContextResolver::of(state).resolve(&turn.entries[index].context, agent).expect("everything it names is here").items);
        let mut under_way = state.clone();
        under_way.turn = Some(turn.clone());
        assert_eq!(seen(&under_way, 1), ["earlier", "question", "prompt:step 1", "first", "prompt:step 2"]);
        state.transcript.extend(turn.messages().cloned().collect());
        assert_eq!(state.transcript.digest(), turn.work.context.transcript, "the turn's digests are those of the transcript");
        assert_eq!(seen(&state, 0), ["earlier", "question", "prompt:step 1"]);
        assert_eq!(seen(&state, 1), ["earlier", "question", "prompt:step 1", "first", "prompt:step 2"]);
    }

    #[test]
    fn the_tools_offered_are_resolved_from_their_set() {
        use crate::tool::ToolParameters;
        let mut state = AgentChannelState::default();
        let offered = vec![ToolDefinition { name: "echo".to_string(), description: "echoes".to_string(), parameters: ToolParameters::object([("text", ToolParameters::string("what"))], ["text"]) }];
        let digest = crate::digest::tools(&offered);
        state.tool_sets.keep(digest, || offered.clone());

        let resolver = ContextResolver::of(&state);
        let with = |tools| ContextRef { tools, ..ContextRef::default() };
        assert_eq!(resolver.resolve(&with(Some(digest)), ActorId::None).expect("the set is kept").tools, offered);
        assert!(resolver.resolve(&with(None), ActorId::None).expect("no tools offered").tools.is_empty());
        assert!(resolver.resolve(&with(Some(prompt("never recorded"))), ActorId::None).is_none(), "a set that was not kept");
    }

    #[test]
    fn a_system_message_stays_where_it_is_only_where_the_model_keeps_it() {
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { role: MessageRole::User, ..text("question") });
        state.transcript.push(Message { role: MessageRole::System, ..text("an old instruction") });

        let roles = |system_messages| {
            let mut state = state.clone();
            state.prompts.keep(prompt("system prompt"), || "system prompt".to_string());
            begin(&mut state, ModelProfile { system_messages, ..ModelProfile::default() });
            state.turn.as_mut().unwrap().work.context.prompts.push(PromptRef { text: prompt("system prompt"), role: MessageRole::System, after: None, classification: None });
            place(&mut state, MessageRole::System, "instruction");
            let next = state.turn.as_ref().unwrap().work.context.clone();
            ContextResolver::of(&state).resolve(&next, ActorId::None).expect("everything it names is here")
                .items.iter().map(ContextItem::role).collect::<Vec<_>>()
        };
        use MessageRole::{System, User};
        assert_eq!(roles(SystemMessages::OnlyAtStart), [System, User, User, User], "only the system prompt stays one");
        assert_eq!(roles(SystemMessages::Anywhere), [System, User, System, System]);
        assert_eq!(SystemMessages::default(), SystemMessages::OnlyAtStart);
    }
}
