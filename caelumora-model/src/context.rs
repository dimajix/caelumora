//! What an agent's model is given, and what is recorded of it (issues 06,
//! 47, 49, 63; see `docs/AGENT-TURN.md`).
//!
//! What is stored are facts: the agent's `Transcript`, and the text of every
//! prompt part once by digest. What the model is given is not stored but
//! resolved from them:
//!
//! - A turn holds a [`ContextFrame`]: the transcript it began with, the
//!   prompt parts its `ContextProvider` placed in it, and the rules of the
//!   agent's model ([`ModelProfile`]).
//! - With every message of the turn a [`ContextRef`] is recorded, which is
//!   the frame as it stood for that message.
//! - The [`ContextResolver`] resolves both against the stored data, in one
//!   implementation: a frame for the next model call of the turn under way,
//!   a reference for a message produced earlier. What the model was given
//!   and what is told as given therefore cannot diverge.

use crate::actor::ActorId;
use crate::agent::{AgentChannelState, TurnState};
use crate::digest::Digest;
use crate::message::{Message, MessageRole, Transcript, TranscriptEntry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A prompt part an agent's model was given besides the conversation - the
/// system prompt, notes, ... (issue 47): the digest of its text, the role it
/// was given as, and the entry of the transcript it followed, `None` if it
/// came before everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRef {
    pub text: Digest,
    pub role: MessageRole,
    pub after: Option<Digest>,
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
    /// digest of `text`, by which the part is kept once and referred to.
    Prompt { digest: Digest, role: MessageRole, text: String },
}

/// Knowledge from another place an agent's model was given (issue 17): what
/// was read, from which channel, as the digest of the state read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRef {
    pub channel: crate::channel::ChannelId,
    pub source: KnowledgeSource,
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
    /// memory chain. Recorded with the channel it was read in.
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
}

impl ModelProfile {
    /// `message` as the model of `agent` is given it; `in_turn` says whether
    /// the turn under way produced the message.
    pub fn given(&self, message: Message, agent: ActorId, in_turn: bool) -> Message {
        self.reasoning_history.given(message, agent, in_turn)
    }
}

/// A prompt part as a `ContextProvider` places it: its text, the role it is
/// given in, and the transcript entry it follows - `None`: before
/// everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPart {
    pub role: MessageRole,
    pub text: String,
    pub after: Option<Digest>,
}

/// What the model of a turn is given, as references (issue 49): the
/// transcript and the prompt parts its `ContextProvider` gave it, and the
/// rules of the model, fixed when the turn begins. The messages the turn
/// produces follow it. Resolved by the [`ContextResolver`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFrame {
    /// The digest of the agent's transcript the model is given.
    pub transcript: Option<Digest>,
    pub prompts: Vec<PromptRef>,
    pub profile: ModelProfile,
}

impl ContextFrame {

    /// The reference recorded for a message the turn produced after
    /// `turn_entries` messages of its own, when the transcript before it had
    /// the digest `transcript` and `knowledge` had been read.
    pub fn reference(&self, transcript: Option<Digest>, turn_entries: usize, knowledge: Vec<KnowledgeRef>) -> ContextRef {
        ContextRef { transcript, prompts: self.prompts.clone(), knowledge, profile: self.profile, turn_entries }
    }
}

/// What an agent's model saw when it produced a message (issue 06): the
/// digest of its `Transcript` before the message - `None` if it was
/// empty - the prompt parts of that turn, each where it was placed (issue
/// 47), the knowledge from other places read before the message was
/// produced (issue 17), and the rules of the model (issue 63). The frame of
/// the turn as it stood for that message; resolved by the
/// [`ContextResolver`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRef {
    /// The digest of the agent's transcript before the message.
    pub transcript: Option<Digest>,
    pub prompts: Vec<PromptRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<KnowledgeRef>,
    /// The rules the model was given its context by.
    pub profile: ModelProfile,
    /// How many of the transcript's last entries the turn that produced the
    /// message had produced before it: the ones given as the turn's own.
    pub turn_entries: usize,
}

/// A context resolved (issues 06, 47): what the model of an agent was given
/// for a message, in the order it was given it, with the reference it was
/// resolved from (issue 119).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// What the history recorded for the message.
    pub reference: ContextRef,
    pub items: Vec<ContextItem>,
}

/// Resolves frames and references against an agent's stored data in a
/// channel - its transcript and the texts of its prompt parts - into what
/// its model is given. One implementation for the model call and for
/// telling afterwards what the model saw.
#[derive(Debug, Clone, Copy)]
pub struct ContextResolver<'a> {
    transcript: &'a Transcript,
    prompts: &'a BTreeMap<Digest, String>,
}

impl<'a> ContextResolver<'a> {
    /// The resolver over the stored data of `state`.
    pub fn of(state: &'a AgentChannelState) -> Self {
        Self { transcript: &state.transcript, prompts: &state.prompts }
    }

    /// What the model of `agent` is given for the next step of `turn`: the
    /// frame the turn began with, then the messages the turn produced so
    /// far. `None` if the frame names what is not here.
    pub fn current(&self, turn: &TurnState, agent: ActorId) -> Option<Vec<ContextItem>> {
        let frame = &turn.frame;
        let before = self.entries(frame.transcript.as_ref())?;
        let during = turn.messages.iter().map(|m| (m, None));
        self.assemble(&frame.prompts, &frame.profile, agent, before, during)
    }

    /// What the model of `agent` was given for the message `reference` was
    /// recorded for: the transcript before the message, of which the last
    /// `turn_entries` are the turn's own, with the prompt parts where they
    /// were placed. `None` if what it names is not here - a transcript that
    /// is not a prefix of this one, a prompt that was never recorded, or
    /// one whose entry is not in the prefix.
    pub fn resolve(&self, reference: &ContextRef, agent: ActorId) -> Option<ResolvedContext> {
        let entries = self.entries(reference.transcript.as_ref())?;
        let turn_start = entries.len().checked_sub(reference.turn_entries)?;
        let (before, during) = entries.split_at(turn_start);
        let during = during.iter().map(|e| (&e.message, Some(e.digest)));
        let items = self.assemble(&reference.prompts, &reference.profile, agent, before, during)?;
        Some(ResolvedContext { reference: reference.clone(), items })
    }

    /// The transcript `digest` names; `None` for the empty transcript.
    fn entries(&self, digest: Option<&Digest>) -> Option<&'a [TranscriptEntry]> {
        match digest {
            Some(digest) => self.transcript.prefix(digest),
            None => Some(&[]),
        }
    }

    /// The items the model of `agent` is given: the entries `before` the
    /// turn, then the turn's messages `during` - each with its digest once
    /// it is in the transcript - as `profile` gives them, with every part of
    /// `prompts` after the entry it follows. `None` if a part's text is not
    /// kept or its entry is not there.
    fn assemble<'m>(
        &self,
        prompts: &[PromptRef],
        profile: &ModelProfile,
        agent: ActorId,
        before: &[TranscriptEntry],
        during: impl Iterator<Item = (&'m Message, Option<Digest>)>,
    ) -> Option<Vec<ContextItem>> {
        let mut items = Vec::with_capacity(before.len() + prompts.len());
        let mut placed = 0;
        let mut place = |after: Option<Digest>, items: &mut Vec<ContextItem>| -> Option<()> {
            for p in prompts.iter().filter(|p| p.after == after) {
                let text = self.prompts.get(&p.text)?.clone();
                items.push(ContextItem::Prompt { digest: p.text, role: p.role, text });
                placed += 1;
            }
            Some(())
        };
        place(None, &mut items)?;
        for entry in before {
            items.push(ContextItem::Conversation { message: profile.given(entry.message.clone(), agent, false) });
            place(Some(entry.digest), &mut items)?;
        }
        for (message, digest) in during {
            items.push(ContextItem::Conversation { message: profile.given(message.clone(), agent, true) });
            if digest.is_some() {
                place(digest, &mut items)?;
            }
        }
        (placed == prompts.len()).then_some(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;
    use crate::digest::prompt;
    use crate::message::MessageId;

    fn text(content: &str) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::default() }
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
            state.prompts.insert(prompt(part), part.to_string());
        }
        let reference = ContextRef {
            transcript: whole,
            prompts: vec![
                PromptRef { text: prompt("system"), role: MessageRole::System, after: None },
                PromptRef { text: prompt("notes"), role: MessageRole::User, after: before_question },
            ],
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

    #[test]
    fn a_turn_under_way_is_resolved_as_its_messages_will_be_recorded() {
        let agent = ActorId::Agent(AgentId::new());
        let own = |content: &str| Message { sender_id: agent, reasoning: Some(format!("why {content}")), ..text(content) };
        let mut state = AgentChannelState::default();
        state.transcript.push(own("earlier"));
        let before_question = state.transcript.push(text("question"));
        state.prompts.insert(prompt("notes"), "notes".to_string());

        let mut frame = ContextFrame { transcript: state.transcript.digest(), ..ContextFrame::default() };
        frame.prompts.push(PromptRef { text: prompt("notes"), role: MessageRole::User, after: before_question });
        let turn = TurnState { frame: frame.clone(), messages: vec![own("first"), own("second")], ..TurnState::default() };
        let current = ContextResolver::of(&state).current(&turn, agent).expect("everything it names is here");
        assert_eq!(contents(&current), ["earlier", "prompt:notes", "question", "first", "second"]);
        let reasoning = current.iter().map(|item| matches!(item, ContextItem::Conversation { message } if message.reasoning.is_some())).collect::<Vec<_>>();
        assert_eq!(reasoning, [false, false, false, true, true], "the turn's own reasoning, not an earlier turn's");

        // Once taken over, the second message's reference resolves to what
        // the turn gave the model for it.
        let before = state.transcript.extend(turn.messages.clone());
        let reference = frame.reference(before[1], 1, Vec::new());
        let seen = ContextResolver::of(&state).resolve(&reference, agent).expect("everything it names is here");
        assert_eq!(contents(&seen.items), ["earlier", "prompt:notes", "question", "first"]);
    }
}
