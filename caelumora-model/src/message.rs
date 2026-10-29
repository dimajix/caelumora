//! Messages: what an actor says in a channel - its text, its reasoning, its attachments, its tool
//! calls and results -, with the rules by which a message is heard, published, modified,
//! reclassified, fitted into its channel and taken back, and what a model's calls used
//! (`TokenUsage`).
use crate::actor::ActorId;
use crate::content::ContentRef;
use crate::confidentiality::{Classification, Visibility};
use crate::digest::Digest;
use crate::store::query::FieldValue;
use crate::tool::{ToolCall, ToolCallResult};
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::iter;
use serde_json::Value;

define_id!(MessageId);


#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// A role as a query compares it: as serde writes it, `"User"`.
impl From<MessageRole> for FieldValue {
    fn from(role: MessageRole) -> Self {
        FieldValue::from(match role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
            MessageRole::Tool => "Tool",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub timestamp: DateTime<Utc>,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub role: MessageRole,
    /// How sensitive the message is and for whom (issue 133): set by its
    /// sender - for a user the channel's, unless they classify it
    /// otherwise; for an agent its turn's label, the join of what the turn
    /// has seen (issue 168) - and checked by the channel it enters, which admits only what may flow
    /// into it. So a message is never classified above its channel; one
    /// classified below is released to a wider audience.
    pub classification: Classification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// What was said together with `content` (issue 282), each classified on its own; a message may
    /// carry attachments alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ContentRef>,
    /// Who sees `reasoning` (issues 61, 133): the model's reasoning may
    /// weigh what the answer would not disclose, so the agent keeps it to
    /// its owner once its turn saw what only they may see.
    #[serde(default, skip_serializing_if = "Visibility::is_channel")]
    pub reasoning_visibility: Visibility,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolCallResult>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// True for a step of a plan under way - its answer, a tool call or
    /// result - rather than the answer itself. Kept like every message, in
    /// the transcript and both histories (issue 258); but no other
    /// participant hears it (`is_heard`), and it is not part of the answer a
    /// `TurnFinished` carries.
    pub internal: bool,
    /// The turn that produced it (issue 235) - an agent's turn, or the work
    /// of an external agent (`Work::turn_id`) - so that which messages of an
    /// agent belong together can be told from the messages alone. `None`
    /// for a user's message, one of the runtime outside a turn, and one
    /// stored before it was kept. Set by the turn as it makes the message
    /// its own, never taken over by a modification (`modify`): a changed
    /// message stays in the turn it was produced in. Not covered by the
    /// digest, like `timestamp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// Participants addressed by `@`-mentions in `content`, resolved once
    /// when the message entered the channel (see `crate::mention`). Stored
    /// rather than re-parsed so that a later rename cannot change whom a past
    /// message addressed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<ActorId>,
    // Usage statistics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_duration: Option<u64>,        // ns
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_duration: Option<u64>,    // ns
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_duration: Option<u64>,         // ns
    /// How many tokens the model's context held in the call that produced
    /// the message (issue 209): what `prompt_tokens` and
    /// `completion_tokens` fill. `None` where it is not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_size: Option<u64>,
    /// When the message was last modified (issue 234), `None` if it never
    /// was. Set where what stands is read - from the changes a chain
    /// recorded after the message - never by its sender, and not covered by
    /// the digest: the change is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
    /// When the message was taken back by its author (issue 234), `None` if
    /// it was not. A message taken back is read without what it said - no
    /// content, reasoning, tool calls or results. Set like `modified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<DateTime<Utc>>,
    /// The message this one tells of, if it is a notice (issue 236): made
    /// by an external agent, in the name of that message's sender, to tell
    /// its remote agent that a message it was given was modified or taken
    /// back. Said by nobody, so never in a history - only in the transcript
    /// of the external agent that gave it. Not covered by the digest: its
    /// content says what was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_of: Option<MessageId>,
    /// What the message was produced from (issues 06, 417, 426), with
    /// `context_frame`: the digest of the transcript its sender's model was
    /// given - or what an external agent's remote agent was given in the
    /// call it came of - before it, `None` if that was empty. Set on what an
    /// agent sends, not on what it keeps in its transcript, and recorded
    /// with the message in its channel's history; only its owner sees it,
    /// so it is never published (`published`). Not covered by the digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_transcript: Option<Digest>,
    /// The digest of the frame around `context_transcript` (`FrameRef`),
    /// which its sender keeps once by digest (`TranscriptStore::frame`).
    /// `None` for a message no turn produced: whether the message was
    /// produced from a context at all (`produced_from`). Set and left out
    /// with `context_transcript`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_frame: Option<Digest>,
}

// No `Default`: a message has no classification that would be right
// everywhere, and a forgotten one must not pass silently (issue 133).
impl Message {
    /// An empty system message classified `classification`, for the other
    /// fields to be filled in with `..`.
    pub fn new(classification: Classification) -> Self {
        Message {
            id: MessageId::default(),
            sender_id: ActorId::None,
            sender_name: "".to_string(),
            role: MessageRole::System,
            classification,
            content: None,
            reasoning: None,
            attachments: Vec::new(),
            reasoning_visibility: Visibility::Channel,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            timestamp: Utc::now(),
            metadata: HashMap::new(),
            internal: false,
            turn_id: None,
            mentions: Vec::new(),
            prompt_tokens: None,
            completion_tokens: None,
            prompt_duration: None,
            completion_duration: None,
            context_transcript: None,
            context_frame: None,
            total_duration: None,
            context_size: None,
            modified: None,
            revoked: None,
            notice_of: None,
        }
    }
}

impl Message {
    /// About how many tokens the message takes in a model's context (issue
    /// 284): its text, its reasoning, its calls and their results, as
    /// `estimated_tokens` estimates. For where no call measured it - on the
    /// message as the model is given it, whose reasoning is there only if
    /// it is given back (`ContextResolver::token_positions`).
    pub fn estimated_tokens(&self) -> u64 {
        let bytes = self.content.as_deref().map_or(0, str::len)
            + self.reasoning.as_deref().map_or(0, str::len)
            + self.tool_calls.iter().map(|call| call.function_name.len() + call.arguments.to_string().len()).sum::<usize>()
            + self.tool_results.iter().map(|result| result.content.len()).sum::<usize>();
        estimated_tokens(bytes)
    }

    /// The message as produced from the context whose transcript has the
    /// digest `transcript` and whose frame has the digest `frame` (issue
    /// 426): what an agent sends of what it said.
    pub fn produced_from_context(self, transcript: Option<Digest>, frame: Digest) -> Message {
        Message { context_transcript: transcript, context_frame: Some(frame), ..self }
    }

    /// What the message was produced from, if it was: the digest of the
    /// transcript before it and that of the frame around it (issue 426).
    pub fn produced_from(&self) -> Option<(Option<Digest>, Digest)> {
        self.context_frame.map(|frame| (self.context_transcript, frame))
    }

    /// The message as its sender publishes it to a channel (issues 54,
    /// 133): every part only its owner sees (`Visibility::Owner`) is
    /// withheld - a tool call or result whose call is the owner's is left
    /// out, a call whose arguments are has them set to `null`, a result
    /// that is is emptied - with the record of the answers to its
    /// questions and what it gave and showed its model - and such reasoning
    /// is dropped, and so is what it was produced from. `None` if nothing is
    /// left to show.
    pub fn published(&self) -> Option<Message> {
        let mut message = Message { context_transcript: None, context_frame: None, ..self.clone() };
        if !message.reasoning_visibility.is_channel() {
            message.reasoning = None;
        }
        message.tool_calls.retain(|c| c.visibility.call.is_channel());
        for call in &mut message.tool_calls {
            if !call.visibility.arguments.is_channel() {
                call.arguments = Value::Null;
            }
        }
        message.tool_results.retain(|r| r.visibility.call.is_channel());
        for result in &mut message.tool_results {
            if !result.visibility.result.is_channel() {
                result.content.clear();
                result.answers = None;
                result.media.clear();
                result.shown.clear();
            }
        }
        let empty = message.content.as_deref().is_none_or(str::is_empty)
            && message.reasoning.as_deref().is_none_or(str::is_empty)
            && message.attachments.is_empty()
            && message.tool_calls.is_empty()
            && message.tool_results.is_empty();
        (!empty).then_some(message)
    }

    /// What a model is given of the message where what it said may not be given (issue 258): which
    /// tools it called and how they ended, without its content, reasoning, attachments, arguments
    /// or what the calls returned, classified `classification`. For a turn given up because its
    /// channel was lowered below it (issue 141), whose content its owner never released. `None` if
    /// nothing is left: no call, no result.
    pub fn names_only(&self, classification: &Classification) -> Option<Message> {
        if self.tool_calls.is_empty() && self.tool_results.is_empty() {
            return None;
        }
        let mut message = Message { content: None, reasoning: None, attachments: Vec::new(), context_transcript: None, context_frame: None, classification: classification.clone(), ..self.clone() };
        for call in &mut message.tool_calls {
            call.arguments = Value::Null;
        }
        for result in &mut message.tool_results {
            result.content.clear();
            result.answers = None;
            result.media.clear();
            result.shown.clear();
            result.classification = classification.clone();
        }
        Some(message)
    }

    /// The content the message carries (issue 282): its attachments, and what its tool results
    /// showed its model.
    pub fn carried(&self) -> impl Iterator<Item = &ContentRef> {
        self.attachments.iter().chain(self.tool_results.iter().flat_map(|result| &result.shown))
    }

    /// The same, to change: the copy a transcript numbers, or a model is given with content.
    pub fn carried_mut(&mut self) -> impl Iterator<Item = &mut ContentRef> {
        self.attachments.iter_mut().chain(self.tool_results.iter_mut().flat_map(|result| &mut result.shown))
    }

    /// Whether a participant hears anything of the message when someone else said it (issue 224):
    /// whether it was said or answered in words or attachments (issue 282) - not a tool message,
    /// not an inner step of a plan, not a message without either. That nobody hears their own
    /// message is the channel's to see to: it delivers nothing back to its sender.
    pub fn is_heard(&self) -> bool {
        matches!(self.role, MessageRole::User | MessageRole::Assistant)
            && !self.internal
            && (self.content.as_deref().is_some_and(|c| !c.is_empty()) || !self.attachments.is_empty())
    }

    /// What a participant hears of the message when someone else said it (`is_heard`), the same
    /// live and on catching up, for every kind of agent: its words and attachments, as a `user`
    /// message without its tool calls and results, which were the sender's. `None` if there is
    /// nothing to hear.
    pub fn as_heard(&self) -> Option<Message> {
        self.is_heard().then(|| Message { role: MessageRole::User, tool_calls: Vec::new(), tool_results: Vec::new(), ..self.clone() })
    }
    /// Modifies the message as it stands by `modified`, the same message as it reads from now on
    /// (issue 234), and returns whether it did: only one not taken back, by the same sender in the
    /// same role. It keeps its place, its id and the time it was said, and takes over what
    /// `modified` says - content, reasoning and attachments, which it may remove and add (issue
    /// 282) - how it is classified, whom it mentions, and when it was modified (`modified`, its
    /// `timestamp`). Its tool calls and results stay those it was said with: a call and its result
    /// would come apart.
    pub fn modify(&mut self, modified: &Message) -> bool {
        let applies = modified.id == self.id && self.revoked.is_none() && modified.sender_id == self.sender_id && modified.role == self.role;
        if applies {
            self.content = modified.content.clone();
            self.reasoning = modified.reasoning.clone();
            self.attachments = modified.attachments.clone();
            self.reasoning_visibility = modified.reasoning_visibility;
            self.classification = modified.classification.clone();
            self.mentions = modified.mentions.clone();
            self.modified = Some(modified.timestamp);
        }
        applies
    }

    /// Classifies the message as it stands `classification` from now on
    /// (issue 133), and returns whether it did: only one not taken back,
    /// which says nothing any more that could be released (issue 239).
    pub fn reclassify(&mut self, classification: &Classification) -> bool {
        let applies = self.revoked.is_none();
        if applies {
            self.classification = classification.clone();
        }
        applies
    }

    /// Fits the message as it stands into a channel lowered to `channel`
    /// (`Classification::fitted_into`), and returns whether that changed it: its classification and
    /// that of each of its attachments, which its channel bounds alike (issue 282).
    pub fn fit_into(&mut self, channel: &Classification) -> bool {
        let mut changed = false;
        for classification in iter::once(&mut self.classification).chain(self.attachments.iter_mut().map(|a| &mut a.classification)) {
            let fitted = classification.fitted_into(channel);
            changed |= fitted != *classification;
            *classification = fitted;
        }
        changed
    }

    /// Takes the message as it stands back, as of `at` (issue 234), and returns whether it did:
    /// only one not taken back already. It keeps its place and its id, and loses what it said -
    /// content, reasoning, attachments, tool calls and results, mentions.
    pub fn revoke(&mut self, at: DateTime<Utc>) -> bool {
        let applies = self.revoked.is_none();
        if applies {
            self.content = None;
            self.reasoning = None;
            self.attachments.clear();
            self.tool_calls.clear();
            self.tool_results.clear();
            self.mentions.clear();
            self.revoked = Some(at);
        }
        applies
    }
}

/// About how many tokens `bytes` of text take in a model's context (issue
/// 284): four bytes a token, rounded up - for where no call measured it.
pub fn estimated_tokens(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX).div_ceil(4)
}

/// What an agent's model calls used (issue 209), summed from the messages
/// they produced, each of which carries the tokens of its call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// How many calls reported their tokens.
    pub calls: u64,
    /// The prompt tokens of all calls: every call is given the whole
    /// context again, so this is what was consumed, not what is held.
    pub prompt_tokens: u64,
    /// The answer tokens of all calls.
    pub completion_tokens: u64,
    /// How many tokens the context of the last call held; `None` if not
    /// known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_size: Option<u64>,
    /// How many tokens the context holds (issue 284): as counted
    /// (`TokenUsage::counted`), the last call's prompt and answer; after a
    /// compaction, the new branch as estimated then
    /// (`AgentChannelState::compact`, issue 390). What was added since is
    /// not in it.
    pub context_tokens: u64,
}

impl TokenUsage {
    /// `usage` with the call that produced `message` counted, if it reported
    /// its tokens; `usage` as it is otherwise. `None`: nothing counted yet.
    pub fn counted(usage: Option<Self>, message: &Message) -> Option<Self> {
        if message.prompt_tokens.is_none() && message.completion_tokens.is_none() {
            return usage;
        }
        let prompt = message.prompt_tokens.unwrap_or(0);
        let completion = message.completion_tokens.unwrap_or(0);
        let usage = usage.unwrap_or(Self { calls: 0, prompt_tokens: 0, completion_tokens: 0, context_size: None, context_tokens: 0 });
        Some(Self {
            calls: usage.calls + 1,
            prompt_tokens: usage.prompt_tokens + prompt,
            completion_tokens: usage.completion_tokens + completion,
            context_size: message.context_size,
            context_tokens: prompt + completion,
        })
    }
}

#[derive(Debug, Clone)]
pub enum MessageEvent {
    /// A partial piece of a message (e.g., a single sentence or a chunk of tokens)
    Partial(Message),
    /// A complete message that is now ready to be routed/processed
    Complete(Message),
    /// A signal that this agent has finished its current turn
    EndOfTurn(Vec<Message>),
}


/// The tool calls among `messages` that ran - their results have a start
/// (issue 145) - each with its result and the call as the model made it, if
/// it is there. What a turn given up is told to have done.
pub fn executed_calls<'a>(messages: &[&'a Message]) -> Vec<(Option<&'a ToolCall>, &'a ToolCallResult)> {
    let calls: Vec<&ToolCall> = messages.iter().flat_map(|m| &m.tool_calls).collect();
    messages.iter()
        .flat_map(|m| &m.tool_results)
        .filter(|result| result.started_at.is_some())
        .map(|result| (calls.iter().copied().find(|c| c.id.is_some() && c.id == result.id), result))
        .collect()
}


/// Messages for the tests of this crate and of others (issue 446): the
/// store suite, which every backend's crate runs, writes them.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;
    use uuid::Uuid;

    /// A message from a user with id `id`.
    pub fn make_message(id: MessageId) -> Message {
        Message {
            id,
            sender_id: ActorId::None,
            sender_name: "tester".to_string(),
            role: MessageRole::User,
            content: Some("hello".to_string()),
            timestamp: Utc::now(),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }
    }

    /// The message id `n`.
    pub fn id(n: u128) -> MessageId {
        MessageId(Uuid::from_u128(n))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use super::testing::{id, make_message};
    use crate::content::testing::make_content_ref;
    use crate::tool::ToolVisibility;
    use chrono::Duration;
    use crate::confidentiality::Confidentiality;
    use crate::store::query::meets_exactly_its_equals;

    #[test]
    fn a_role_meets_exactly_its_serialized_form() {
        meets_exactly_its_equals(&[MessageRole::User, MessageRole::Assistant, MessageRole::System, MessageRole::Tool]);
    }

    fn call(id: &str, visibility: ToolVisibility) -> ToolCall {
        ToolCall { id: Some(id.to_string()), function_name: "f".to_string(), arguments: serde_json::json!({"text": "secret"}), visibility, presentation: Default::default() }
    }

    fn result(id: &str, visibility: ToolVisibility) -> ToolCallResult {
        let started_at = Utc::now();
        ToolCallResult {
            id: Some(id.to_string()), tool_name: "f".to_string(), content: "secret".to_string(), status: Default::default(), visibility, classification: Classification::CHANNEL_DEFAULT,
            started_at: Some(started_at), finished_at: Some(started_at + Duration::milliseconds(250)), answers: None, approval: Vec::new(), media: Vec::new(), shown: Vec::new(),
        }
    }

    /// Issue 282: what is attached is said with the text, and heard,
    /// published, changed and taken back with it - a message may carry it
    /// alone; what a tool gave or showed its model is the result's, withheld
    /// with it.
    #[test]
    fn what_is_attached_goes_with_what_was_said() {
        let screenshot = make_content_ref("screen.png", b"png");
        let alone = Message { content: None, attachments: vec![screenshot.clone()], ..make_message(id(1)) };
        assert!(alone.is_heard(), "an attachment alone is something to hear");
        assert_eq!(alone.as_heard().expect("heard").attachments, alone.attachments, "heard with what was attached");
        assert_eq!(alone.published().expect("an attachment alone is something to show").attachments, alone.attachments);
        assert!(alone.names_only(&Classification::PUBLIC).is_none(), "nothing of it where what was said may not be given");

        let other = make_content_ref("better.png", b"better");
        let mut message = alone.clone();
        assert!(message.modify(&Message { attachments: vec![other.clone()], ..alone.clone() }));
        assert_eq!(message.attachments, [other], "removed and added as the new version says");
        let lowered = Classification::PUBLIC;
        let mut fitted = message.clone();
        assert!(fitted.fit_into(&lowered), "a lowered channel fits it");
        assert_eq!((&fitted.classification, &fitted.attachments[0].classification), (&lowered, &lowered), "and what is attached to it, which the channel bounds alike");
        assert!(!fitted.fit_into(&lowered), "fitted once");

        assert!(message.revoke(Utc::now()));
        assert!(message.attachments.is_empty() && !message.is_heard(), "taken back with what was said");

        let seen = |visibility| ToolCallResult { media: vec![make_content_ref("chart.png", b"chart")], shown: vec![screenshot.clone()], ..result("1", visibility) };
        let results = Message { role: MessageRole::Tool, content: None, tool_results: vec![seen(ToolVisibility::SHOWN), seen(ToolVisibility::WITHOUT_RESULT)], ..make_message(id(2)) };
        let published = results.published().expect("both calls are public");
        assert_eq!((published.tool_results[0].media.len(), published.tool_results[0].shown.len()), (1, 1), "a public result with what it gave and showed");
        assert!(published.tool_results[1].media.is_empty() && published.tool_results[1].shown.is_empty(), "withheld with the result");
        let given = results.names_only(&Classification::PUBLIC).expect("results");
        assert!(given.tool_results.iter().all(|r| r.media.is_empty() && r.shown.is_empty()), "how the calls ended, not what they gave");
        let for_the_model = results.tool_results[0].for_the_model();
        assert!(for_the_model.media.is_empty() && for_the_model.shown.is_empty(), "a model is given them as what they are, not as text");
    }

    #[test]
    fn a_message_is_heard_as_what_was_said_in_words_without_the_tool_traffic() {
        let text = |role: MessageRole, content: Option<&str>| Message { role, content: content.map(str::to_string), ..make_message(MessageId::new()) };
        let answer = Message { tool_calls: vec![call("1", ToolVisibility::SHOWN)], ..text(MessageRole::Assistant, Some("Let me check.")) };

        assert!(answer.is_heard());
        let heard = answer.as_heard().expect("an answer in words is heard");
        assert_eq!((heard.role, heard.content.as_deref()), (MessageRole::User, Some("Let me check.")), "as a user's");
        assert!(heard.tool_calls.is_empty(), "without the sender's call");
        assert_eq!((heard.id, heard.sender_id), (answer.id, answer.sender_id), "the same message, of the same sender");

        let tool = Message { tool_results: vec![result("1", ToolVisibility::SHOWN)], ..text(MessageRole::Tool, Some("done")) };
        let internal = Message { internal: true, ..text(MessageRole::Assistant, Some("step")) };
        for unheard in [tool, internal, text(MessageRole::Assistant, Some("")), text(MessageRole::Assistant, None), text(MessageRole::System, Some("note"))] {
            assert!(!unheard.is_heard() && unheard.as_heard().is_none(), "nothing to hear in {:?}", unheard.role);
        }
    }

    #[test]
    fn a_message_is_published_without_what_only_its_owner_sees() {
        let message = Message {
            content: None,
            tool_calls: vec![call("1", ToolVisibility::SHOWN), call("2", ToolVisibility::CALL_ONLY), call("3", ToolVisibility::HIDDEN), call("4", ToolVisibility::WITHOUT_RESULT)],
            ..make_message(MessageId::new())
        };
        let published = message.published().expect("public and call-only calls are left");
        let ids: Vec<_> = published.tool_calls.iter().map(|c| c.id.clone().unwrap()).collect();
        assert_eq!(ids, ["1", "2", "4"], "the hidden call is gone");
        assert_eq!(published.tool_calls[0].arguments, message.tool_calls[0].arguments, "public as it is");
        assert_eq!(published.tool_calls[1].arguments, Value::Null, "call-only without arguments");
        assert_eq!(published.tool_calls[2].arguments, message.tool_calls[3].arguments, "with its arguments where only its result is the owner's");
        assert_eq!(published.id, message.id, "the same message, for the context panel to find");

        use crate::approval::ApprovalDecision;
        use crate::question::{Addressee, Answers, Question, Questions};
        let answered = |result: ToolCallResult| {
            let questions = Questions { questions: vec![Question { header: "Name".to_string(), question: "Who?".to_string(), choices: Vec::new(), multi_select: false }], ask: vec![Addressee::Owner], sensitive: false };
            let approval = vec![ApprovalDecision { by: ActorId::None, name: "alice".to_string(), by_rule: None, at: Utc::now() }];
            ToolCallResult { answers: Some(Answers::declined(questions, ActorId::None, "alice".to_string())), approval, ..result }
        };
        let results = Message {
            role: MessageRole::Tool,
            content: None,
            tool_results: vec![answered(result("1", ToolVisibility::SHOWN)), answered(result("2", ToolVisibility::CALL_ONLY)), answered(result("3", ToolVisibility::HIDDEN)), answered(result("4", ToolVisibility::WITHOUT_RESULT))],
            ..make_message(MessageId::new())
        };
        let published = results.published().expect("public and call-only results are left");
        let contents: Vec<_> = published.tool_results.iter().map(|r| (r.content.as_str(), r.answers.is_some())).collect();
        assert_eq!(contents, [("secret", true), ("", false), ("", false)], "the hidden result is gone, the call-only one and the one without its result empty, the record of its answers with it");
        assert!(published.tool_results.iter().all(|r| r.approval.len() == 1), "who approved the call stays with it, its result withheld or not: every user was told (issue 419)");
        let for_the_model = results.tool_results[0].for_the_model();
        assert!(for_the_model.answers.is_none() && for_the_model.approval.is_empty(), "a model is never given the record");
        let ran = |r: &ToolCallResult| (r.started_at, r.finished_at);
        assert_eq!(ran(&published.tool_results[1]), ran(&results.tool_results[1]), "when it ran is the call's, not the result's");
    }

    #[test]
    fn only_calls_that_started_count_as_executed() {
        let calls = Message { tool_calls: vec![call("1", ToolVisibility::SHOWN), call("2", ToolVisibility::SHOWN)], ..make_message(MessageId::new()) };
        let never_ran = ToolCallResult { started_at: None, finished_at: None, ..result("2", ToolVisibility::SHOWN) };
        let results = Message { tool_results: vec![result("1", ToolVisibility::SHOWN), never_ran], ..make_message(MessageId::new()) };
        let executed = executed_calls(&[&calls, &results]);
        assert_eq!(executed.len(), 1);
        assert_eq!((executed[0].0.and_then(|c| c.id.as_deref()), executed[0].1.id.as_deref()), (Some("1"), Some("1")), "with the call it answers");
    }

    #[test]
    fn a_message_with_nothing_left_to_show_is_not_published() {
        let hidden_only = Message { content: None, tool_calls: vec![call("1", ToolVisibility::HIDDEN)], ..make_message(MessageId::new()) };
        assert!(hidden_only.published().is_none());
        let private_reasoning = Message {
            content: None,
            reasoning: Some("the note says teal".to_string()),
            reasoning_visibility: Visibility::Owner,
            ..make_message(MessageId::new())
        };
        assert!(private_reasoning.published().is_none(), "private reasoning alone is nothing to show");
        let with_text = Message { tool_calls: vec![call("1", ToolVisibility::HIDDEN)], ..make_message(MessageId::new()) };
        let published = with_text.published().expect("the text is left");
        assert!(published.tool_calls.is_empty());
        assert_eq!(published.content.as_deref(), Some("hello"));
    }

    #[test]
    fn of_a_message_withheld_only_which_tools_it_called_is_given() {
        use crate::tool::{ToolCall, ToolCallResult, ToolResultStatus};
        let secret = Classification::general(Confidentiality::Confidential);
        let call = ToolCall { id: Some("1".to_string()), function_name: "read".to_string(), arguments: serde_json::json!({ "file": "plans" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let calling = Message { content: Some("let me look".to_string()), reasoning: Some("hm".to_string()), tool_calls: vec![call.clone()], ..Message::new(secret.clone()) };
        let result = ToolCallResult::of(&call, ToolResultStatus::Ok, "the plans".to_string(), secret.clone());
        let answered = Message { role: MessageRole::Tool, tool_results: vec![result], ..Message::new(secret.clone()) };

        let given = calling.names_only(&Classification::PUBLIC).expect("it called a tool");
        assert_eq!((given.content, given.reasoning, given.classification), (None, None, Classification::PUBLIC));
        assert_eq!((given.tool_calls[0].function_name.as_str(), &given.tool_calls[0].arguments), ("read", &Value::Null));
        let given = answered.names_only(&Classification::PUBLIC).expect("a result");
        assert_eq!((given.tool_results[0].status, given.tool_results[0].content.as_str()), (ToolResultStatus::Ok, ""), "how it ended, not what it returned");
        assert_eq!(given.tool_results[0].classification, Classification::PUBLIC);
        let words = Message { content: Some("said".to_string()), ..Message::new(secret) };
        assert!(words.names_only(&Classification::PUBLIC).is_none(), "nothing left of words alone");
    }

    #[test]
    fn labels_are_stored_only_where_they_are_not_public() {
        let public = serde_json::to_value(call("1", ToolVisibility::SHOWN)).unwrap();
        assert!(public.get("visibility").is_none());
        let hidden = serde_json::to_value(call("1", ToolVisibility::HIDDEN)).unwrap();
        assert_eq!(hidden["visibility"], serde_json::json!({"call": "owner", "arguments": "owner", "result": "owner"}));
        let stored: ToolCall = serde_json::from_value(serde_json::json!({"id": "1", "function_name": "f", "arguments": {}})).unwrap();
        assert!(stored.visibility.is_shown(), "a call without labels is public");
        let plain = serde_json::to_value(make_message(MessageId::new())).unwrap();
        assert!(plain.get("reasoning_visibility").is_none());
        assert!(plain.get("turn_id").is_none(), "a message of no turn says nothing of one");
        assert!(plain.get("attachments").is_none(), "nor of attachments, where it has none");
    }

    #[test]
    fn a_message_is_modified_and_taken_back_as_it_stands() {
        let turn = TurnId::new();
        let said = Message { tool_calls: vec![call("1", ToolVisibility::SHOWN)], turn_id: Some(turn), ..make_message(id(1)) };
        let public = Classification::general(Confidentiality::Public);
        let when = said.timestamp + Duration::minutes(5);
        let modified = Message { content: Some("hello, corrected".to_string()), classification: public.clone(), timestamp: when, tool_calls: Vec::new(), turn_id: None, ..said.clone() };

        let mut message = said.clone();
        assert!(message.modify(&modified));
        assert_eq!(message.content.as_deref(), Some("hello, corrected"));
        assert_eq!(message.classification, public, "how it is classified, too");
        assert_eq!((message.timestamp, message.modified), (said.timestamp, Some(when)), "said when it was, modified when it was changed");
        assert_eq!(message.tool_calls.len(), 1, "its calls are not taken over");
        assert_eq!(message.turn_id, Some(turn), "nor its turn: it stays in the one it was produced in");

        let mut other = said.clone();
        assert!(!other.modify(&Message { sender_id: ActorId::System, ..modified.clone() }), "by another sender");
        assert!(!other.modify(&Message { role: MessageRole::Assistant, ..modified.clone() }), "in another role");
        assert!(!other.modify(&Message { id: id(2), ..modified.clone() }), "another message");
        assert_eq!(other.content, said.content, "left as it was");

        let released = Classification::PUBLIC;
        let mut other = said.clone();
        assert!(other.reclassify(&released));
        assert_eq!((other.classification, other.modified), (released.clone(), None), "reclassified, not modified");

        assert!(message.revoke(when));
        assert_eq!(message.revoked, Some(when));
        assert!(message.content.is_none() && message.tool_calls.is_empty(), "nothing of what it said");
        assert!(!message.is_heard(), "nobody hears it");
        let before = serde_json::to_value(&message).unwrap();
        assert!(!message.modify(&modified) && !message.revoke(when) && !message.reclassify(&released), "nothing changes it any more");
        assert_eq!(serde_json::to_value(&message).unwrap(), before);
    }
}
