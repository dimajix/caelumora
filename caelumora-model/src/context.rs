//! What an agent's model is given, and what is recorded of it (issues 06,
//! 47, 49, 63; see `docs/AGENT-TURN.md`).
//!
//! What is stored are facts: the agent's `Transcript`, and the text of every
//! prompt part, every set of tools offered and every frame once by digest.
//! What the model is given is not stored but resolved from them:
//!
//! - A turn holds a [`ContextRef`] for its next message: the transcript so
//!   far and the frame around it ([`FrameRef`]) - the prompt parts placed
//!   in it, the tools offered, the knowledge read and the rules of the
//!   agent's model ([`ModelProfile`]). Every message it says goes into the
//!   transcript at once, and into the history with the reference as it
//!   stood for that message (issue 260): the digest of its transcript and
//!   that of its frame, which is kept once (issue 426).
//! - The [`ContextResolver`] resolves a reference against the transcript
//!   and the prompt texts - for the next model call as for a message produced
//!   earlier. What the model was given and what is told as given therefore
//!   cannot diverge. It reads the transcript alone: the history is the
//!   record of what was said and what it was produced from, and is never
//!   read to build what the model is given (issue 387).

use crate::actor::ActorId;
use crate::agent::{AgentChannelState, AgentId};
use crate::chain::{Node, Position};
use crate::confidentiality::Classification;
use crate::content::ContentRef;
use crate::digest::Digest;
use crate::kept::Kept;
use crate::message::{estimated_tokens, Message, MessageId, MessageRole};
use crate::prompt_catalogue::{context, PromptCatalogue, PromptError, PromptTexts, PromptVariant, CORE};
use std::sync::Arc;
use crate::store::{EntityKind, Stored};
use crate::tool::ToolSet;
use crate::transcript::{as_they_stand, Transcript, TranscriptEntry};
use crate::turn::GivenUpReason;
use crate::value::time_zone::TimeZone;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::iter;
use uuid::Uuid;
use crate::channel::ChannelId;
use crate::mention::handle_of;
use crate::session::SessionId;

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
#[expect(clippy::large_enum_variant, reason = "a message is the usual element, and boxing it would cost an allocation per element")]
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
    pub channel: ChannelId,
    /// The session of `channel`: whether it was read beyond the agent's own.
    pub session: SessionId,
    pub source: KnowledgeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
}

/// What a [`KnowledgeRef`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnowledgeSource {
    /// The notes an agent published in the channel; the digest of its note
    /// chain, `None` while it had none.
    Notes { agent: AgentId, digest: Option<Digest> },
    /// An agent's private notes in the channel (issue 51), read by that
    /// agent; the digest of its private note chain.
    PrivateNotes { agent: AgentId, digest: Option<Digest> },
    /// The channel's history; the digest of it when it was read - up to
    /// where - and, for a reader who joined later with a limit, the position
    /// their view began at (issues 72, 417). `None`: all of it.
    History {
        digest: Option<Digest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Position>,
    },
    /// An agent's memory (issue 62), read by that agent; the digest of its
    /// memory chain. Recorded once for every channel the entries recalled
    /// were written in, so that knowledge from another session can be told
    /// (issue 134).
    Memory { agent: AgentId, digest: Option<Digest> },
    /// An entity of whatever kind (issue 323), as its kind records what was
    /// read of it: the files of a workspace (issue 251), read through the
    /// channel it is used in, with the digest of a file's content as it
    /// was read - `None` for a listing or a search -; a skill (issue 22),
    /// loaded into the turn, with the digest of its body as it was loaded
    /// (the skills plugin's `skill::digest`).
    Entity { kind: EntityKind, id: Uuid, digest: Option<Digest> },
}

impl KnowledgeSource {
    /// That the entity of kind `T` with `id` was read, as `digest` says.
    pub fn entity<T: Stored>(id: T::Id, digest: Option<Digest>) -> Self {
        Self::Entity { kind: T::KIND, id: id.into(), digest }
    }

    /// The id of the entity of kind `T` this is the knowledge of, and the
    /// digest of what was read; `None` for knowledge of anything else.
    pub fn about<T: Stored>(&self) -> Option<(T::Id, Option<Digest>)> {
        match *self {
            Self::Entity { kind, id, digest } if kind == T::KIND => Some((T::Id::from(id), digest)),
            _ => None,
        }
    }
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
    /// How the model is told who wrote a message of another participant
    /// (issue 135). Read as `none` where a context recorded before has
    /// none: it was given so.
    #[serde(default)]
    pub sender_format: SenderFormat,
    /// How a sender is named in the marker.
    #[serde(default)]
    pub sender_style: SenderStyle,
    /// Which variant of the texts of the prompt catalogue the model is given
    /// (issue 57), as its component names it; `None`: the default. Recorded
    /// with the context, so that what a model saw is worded as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_texts: Option<PromptVariant>,
    /// Whether the marker says when the message was written too (issue 424),
    /// in the time zone of the context (`FrameRef::time_zone`). Never on
    /// the agent's own messages, which carry no marker: a model whose own
    /// messages carry a time begins its answers with one of its own
    /// (`work/message_times/RESULTS.md`, finding 2). Nothing where senders
    /// are not marked.
    pub message_times: bool,
    /// Whether the model is given images (issue 282): an attachment that is
    /// one is given along with its message, else the model is told of it
    /// alone. Read as `false` where a context recorded before has none: it
    /// was given none.
    #[serde(default)]
    pub images: bool,
}

/// Whether a model or remote agent is given the times of messages unless its
/// component is configured otherwise (issue 424): it is - a model uses them,
/// and takes the time of the last message for now without them.
pub fn message_times_by_default() -> bool {
    true
}

/// The time the explanation of the markers shows as an example (issue 424),
/// in the form `TimeZone::format` writes it.
const TIME_EXAMPLE: &str = "Sun 18 Oct 2026, 14:32";

impl ModelProfile {
    /// `message` as the model of `agent` is given it; `in_turn` says whether
    /// the turn under way produced the message, `time_zone` is the one its
    /// time is written in.
    pub fn given(&self, message: Message, agent: ActorId, in_turn: bool, time_zone: TimeZone) -> Message {
        let message = self.reasoning_history.given(message, agent, in_turn);
        self.frame(message, agent, time_zone)
    }

    /// Whether the model is given `attachment` along with its message rather
    /// than told of it alone (issue 282): an image, to a model that sees.
    pub fn gives_along(&self, attachment: &ContentRef) -> bool {
        self.images && attachment.is_image()
    }

    /// How what the model is given of `message` is classified: the message,
    /// its tool results, and the attachments it is given along (issue 282) -
    /// what sets the label of what the model produces from it.
    pub fn classifications_given<'m>(&self, message: &'m Message) -> impl Iterator<Item = &'m Classification> {
        let profile = *self;
        iter::once(&message.classification)
            .chain(message.tool_results.iter().map(|result| &result.classification))
            .chain(message.attachments.iter().filter(move |attachment| profile.gives_along(attachment)).map(|attachment| &attachment.classification))
    }

    /// `message` framed as the model of `agent` is given it: its attachments
    /// noted after its text (`noted`), whoever said it; then marked with its
    /// sender as `sender_format` and `sender_style` say, and with when it was
    /// written in `time_zone` where `message_times` says so - if another
    /// participant, a user or agent, said it.
    fn frame(&self, mut message: Message, agent: ActorId, time_zone: TimeZone) -> Message {
        if let Some(note) = self.noted(&message.attachments) {
            message.content = Some(match message.content.take().filter(|content| !content.is_empty()) {
                Some(content) => format!("{content}\n{note}"),
                None => note,
            });
        }
        let said_by_another = message.sender_id != agent
            && matches!(message.sender_id, ActorId::User(_) | ActorId::Agent(_))
            && message.role == MessageRole::User;
        if self.sender_format == SenderFormat::None || !said_by_another {
            return message;
        }
        let Some(content) = message.content.as_deref() else { return message };
        let sender = self.sender_style.of(&message.sender_name);
        let at = self.message_times.then(|| time_zone.format(message.timestamp));
        message.content = Some(match (self.sender_format, at) {
            (SenderFormat::None, _) => return message,
            (SenderFormat::Prefix, None) => format!("{sender}: {content}"),
            (SenderFormat::Prefix, Some(at)) => format!("{sender} ({at}): {content}"),
            // Only what would end or open an element is escaped: escaping
            // more hides typos in code from the model (RESULTS.md, 7).
            (SenderFormat::Xml, at) => format!(
                "<message from=\"{}\"{}>{}</message>",
                attribute(&sender),
                at.map(|at| format!(" at=\"{at}\"")).unwrap_or_default(),
                content.replace("</message", "&lt;/message").replace("<message", "&lt;message"),
            ),
        });
        message
    }

    /// The note on `attachments` (issue 282), one line each, the model told
    /// of each what it is, how it names it, and whether it is given it along:
    /// a marker, as the sender's is, in the form of the sender's - an element
    /// with `xml`, else in brackets. `None` without attachments.
    fn noted(&self, attachments: &[ContentRef]) -> Option<String> {
        let lines: Vec<String> = attachments.iter().map(|attachment| {
            let size = attachment.size_shown();
            let shown = self.gives_along(attachment);
            match self.sender_format {
                SenderFormat::Xml => {
                    let reference = attachment.reference().map(|reference| format!(" ref=\"{reference}\"")).unwrap_or_default();
                    format!("<attachment{reference} name=\"{}\" type=\"{}\" size=\"{size}\" shown=\"{}\"/>", attribute(attachment.name.as_str()), attribute(&attachment.media_type), if shown { "yes" } else { "no" })
                }
                SenderFormat::None | SenderFormat::Prefix => {
                    let reference = attachment.reference().map(|reference| format!("{reference}: ")).unwrap_or_default();
                    format!("[attachment {reference}{}, {}, {size}{}]", attachment.name.as_str(), attachment.media_type, if shown { "" } else { ", not shown" })
                }
            }
        }).collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// What a receiver that keeps its own conversation - the remote agent
    /// of an external agent - is given of the active path of `transcript`,
    /// for `agent`: its messages as they stand (`Transcript::messages`) and,
    /// where someone joined or left the channel, what it is told of that
    /// (issue 408), worded by `texts`, each as `given` makes it.
    ///
    /// # Errors
    /// `PromptError` if the text of a notice cannot be had.
    pub fn given_messages(&self, texts: &PromptTexts, transcript: &Transcript, agent: ActorId, time_zone: TimeZone) -> Result<Vec<Message>, PromptError> {
        let mut given = Vec::new();
        for (node, message) in as_they_stand(transcript.entries()) {
            let message = match message {
                Some(message) => Some(message),
                None => participant_notice(texts, &node.entry, &node.digest, agent, self.sender_style)?,
            };
            given.extend(message.map(|message| self.given(message, agent, false, time_zone)));
        }
        Ok(given)
    }

    /// What the model is to be told of how the messages of others are framed
    /// (`frame`) - their senders, and when they were written -, worded by
    /// `texts` (`core.framing`); `None` if they are not (issues 135, 424).
    ///
    /// # Errors
    /// `PromptError` if the text cannot be had.
    pub fn framing_explanation(&self, texts: &PromptTexts) -> Result<Option<String>, PromptError> {
        let format = match self.sender_format {
            SenderFormat::None => return Ok(None),
            SenderFormat::Prefix => "prefix",
            SenderFormat::Xml => "xml",
        };
        let style = match self.sender_style {
            SenderStyle::Handle => "handle",
            SenderStyle::Name => "name",
        };
        texts.render(&format!("{CORE}.framing"), context! { format, style, times => self.message_times, time => TIME_EXAMPLE }).map(Some)
    }

    /// The texts of `catalogue` as the model is given them: in the variant
    /// it names (issue 57).
    pub fn texts(&self, catalogue: &Arc<PromptCatalogue>) -> PromptTexts {
        PromptTexts::new(Arc::clone(catalogue), self.prompt_texts)
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

/// How a model is told who wrote a message (issue 135). Its input has no
/// place for a sender - Ollama drops a `name`, A2A knows only the roles
/// `user` and `agent` - so the sender is marked in the content. Only the
/// messages of other participants are: a model whose own messages carry a
/// marker continues the conversation as a transcript
/// (`work/sender_names/RESULTS.md`). The marker is a claim like any text: a
/// participant can write another's into their own message (issue 150).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderFormat {
    /// Not at all: the model sees the messages of others without a sender.
    #[default]
    None,
    /// Before the content: `@alice: text`.
    Prefix,
    /// Around the content: `<message from="@alice">text</message>`.
    Xml,
}

impl SenderFormat {
    /// How a model or remote agent is given senders unless its component is
    /// configured otherwise: marked before the content - a channel of
    /// several participants does not work without. Unlike the type's
    /// default, which is how a context recorded before it was given.
    pub fn configured_default() -> Self {
        Self::Prefix
    }
}

/// How a sender is named in the marker (issue 135).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderStyle {
    /// The `@` handle, as the roster names it and mentions use it - derived
    /// from the name the message holds, so that a stored transcript reads
    /// the same after a renaming; the name where it gives no handle.
    #[default]
    Handle,
    /// The display name the message holds.
    Name,
}

impl SenderStyle {
    /// How the sender called `name` in a message is marked.
    pub fn of(self, name: &str) -> String {
        match self {
            Self::Handle => match handle_of(name) {
                handle if handle.is_empty() => name.to_string(),
                handle => format!("@{handle}"),
            },
            Self::Name => name.to_string(),
        }
    }
}

/// A prompt part as a `ContextProvider` places it: its text, the role it is
/// given in, and the transcript entry it follows - `None`: before
/// everything - how what it holds besides knowledge is classified, `None`
/// if that is not known (issue 168), and the component it came from, if
/// from one alone (issue 214): what an external agent's actor tells the
/// parts it gives apart by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPart {
    pub role: MessageRole,
    pub text: String,
    pub after: Option<Digest>,
    pub classification: Option<Classification>,
    pub source: Option<String>,
}

/// What an agent's model saw when it produced a message (issue 06): the
/// digest of its `Transcript` before the message - `None` if it was
/// empty -, and the frame around it (issue 426). A turn keeps one for its
/// next message (`Work::context` of its `TurnState`); resolved by the
/// [`ContextResolver`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRef {
    /// The digest of the agent's transcript before the message.
    pub transcript: Option<Digest>,
    pub frame: FrameRef,
}

/// What an agent's model was given around its transcript (issue 426): the
/// prompt parts of the turn, each where it was placed (issue 47), the tools
/// it was offered (issue 137), the knowledge from other places read before
/// the message was produced (issue 17), and the rules of the model (issue
/// 63). Set by the turn, apart from the transcript, which moves on with
/// every message it says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRef {
    pub prompts: Vec<PromptRef>,
    /// The digest of the tool sets the model was offered
    /// (`crate::digest::tool_sets`), `None` if it was offered none.
    pub tools: Option<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<KnowledgeRef>,
    /// The rules the model was given its context by.
    pub profile: ModelProfile,
    /// The time zone the times of messages were written in (issue 424): the
    /// session's as the turn began - recorded, so that what the model was
    /// given is told the same after the session's changed.
    pub time_zone: TimeZone,
}

/// A context resolved (issues 06, 47): what the model of an agent was given
/// for a message, in the order it was given it, and the tools it was offered
/// (issue 137), with the reference it was resolved from (issue 119).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// What the history recorded for the message.
    pub reference: ContextRef,
    pub items: Vec<ContextItem>,
    /// The tools offered, in their sets (issue 194).
    pub tool_sets: Vec<ToolSet>,
}

/// Resolves references against an agent's data in a channel - its
/// transcript, the texts of its prompt parts and the sets of tools it was
/// offered - into what its model is given. One implementation for the model
/// call, for telling afterwards what the model saw, and for measuring how
/// much of its context it fills (issue 284).
#[derive(Debug, Clone, Copy)]
pub struct ContextResolver<'a> {
    transcript: TranscriptSource<'a>,
    prompts: &'a Kept<String>,
    offered_tools: &'a Kept<Vec<ToolSet>>,
    /// The texts of the notices it gives the model (issue 57), each worded
    /// in the variant of the context it resolves.
    catalogue: &'a Arc<PromptCatalogue>,
}

/// What a `ContextResolver` reads the transcript a reference names from
/// (issue 380): the part of it an agent's state holds - its active branch -,
/// or a path loaded from the store.
#[derive(Debug, Clone, Copy)]
enum TranscriptSource<'a> {
    Held(&'a Transcript),
    Path(&'a [Node<TranscriptEntry>]),
}

impl<'a> TranscriptSource<'a> {
    /// The entries of the path `digest` names, up to and including its
    /// entry; `None` if it holds no such entry.
    fn prefix(self, digest: &Digest) -> Option<Vec<&'a Node<TranscriptEntry>>> {
        match self {
            TranscriptSource::Held(transcript) => transcript.prefix(digest),
            TranscriptSource::Path(path) => path.iter().position(|node| node.digest == *digest).map(|at| path[..=at].iter().collect()),
        }
    }
}

/// The items the model is given before the first entry of a transcript, and
/// for each entry what it is given of it - its message, where a turn given
/// up ended, the prompt parts placed after it.
type GivenPath<'a> = (Vec<ContextItem>, Vec<(&'a Node<TranscriptEntry>, Vec<ContextItem>)>);

impl<'a> ContextResolver<'a> {
    /// The resolver over the data of `state`, its transcript as the state
    /// holds it: what the next call of its model and how full its context
    /// is are resolved against - the head of its active branch. Its
    /// notices are worded by `catalogue`.
    pub fn of(state: &'a AgentChannelState, catalogue: &'a Arc<PromptCatalogue>) -> Self {
        Self { transcript: TranscriptSource::Held(&state.transcript), prompts: &state.prompts, offered_tools: &state.offered_tools, catalogue }
    }

    /// The resolver over what was loaded from the store for an older
    /// message's context - which may lie on a branch the state does not
    /// hold, and name texts it no longer holds -: the path `path` of its
    /// transcript (`TranscriptStore::path`, issue 380), the `prompts` and
    /// the `offered_tools` it names (`TranscriptStore::named_by`, issue
    /// 383).
    /// Its notices are worded by `catalogue`.
    pub fn on_path(path: &'a [Node<TranscriptEntry>], prompts: &'a Kept<String>, offered_tools: &'a Kept<Vec<ToolSet>>, catalogue: &'a Arc<PromptCatalogue>) -> Self {
        Self { transcript: TranscriptSource::Path(path), prompts, offered_tools, catalogue }
    }

    /// What the model of `agent` is given, or was given, for the message
    /// `reference` is for, produced in the turn `turn` - the next one of the
    /// turn under way (`Work::context` of its `TurnState`) or one recorded in
    /// the history: the transcript it names, whose messages of `turn` are
    /// the turn's own (`Message::turn_id`, issue 241), with the prompt parts
    /// where they were placed, and the tools it was offered. `None` if what
    /// it names is not here - a transcript that is not a prefix of this
    /// one, a prompt or a set of tools that was never recorded, or a prompt
    /// whose entry is not in the prefix.
    ///
    /// # Errors
    /// `PromptError` if the text of a notice cannot be had.
    pub fn resolve(&self, reference: &ContextRef, turn: TurnId, agent: ActorId) -> Result<Option<ResolvedContext>, PromptError> {
        let Some((mut items, entries)) = self.given_path(reference, turn, agent)? else { return Ok(None) };
        items.extend(entries.into_iter().flat_map(|(_, given)| given));
        reference.frame.profile.place_system_messages(&mut items);
        let tool_sets = match &reference.frame.tools {
            Some(digest) => match self.offered_tools.get(digest) {
                Some(sets) => sets.clone(),
                None => return Ok(None),
            },
            None => Vec::new(),
        };
        Ok(Some(ResolvedContext { reference: reference.clone(), items, tool_sets }))
    }

    /// Where each entry of the transcript `reference` names stands in what
    /// the model of `agent` is given for it, in turn `turn`, in tokens (issue
    /// 284): how many it is given before the entry, and as a last element how
    /// many in all; and whether a call of its model measured any of it.
    /// `None` as for `resolve`.
    ///
    /// Measured before a message of the model of `agent` added where it
    /// stands - after the compaction that began the path's branch
    /// (`TranscriptEntry::Compacted`, issue 387), whose copies and summary
    /// carry the measures of calls made on another branch: its call's
    /// `prompt_tokens`, which count everything the call was given before it,
    /// the system prompt as well. Read from the transcript alone: what is
    /// given to the model is built from it, never from the history.
    /// Elsewhere estimated
    /// (`ContextItem::estimated_tokens`) from what the model is given of
    /// each entry, as `resolve` gives it, a message's reasoning only where
    /// its rule gives it back: on from each measured position, back from
    /// the first; from nothing if none is.
    ///
    /// # Errors
    /// As `resolve`.
    pub fn token_positions(&self, reference: &ContextRef, turn: TurnId, agent: ActorId) -> Result<Option<(Vec<u64>, bool)>, PromptError> {
        let Some((leading, entries)) = self.given_path(reference, turn, agent)? else { return Ok(None) };
        let size = |items: &[ContextItem]| items.iter().map(|item| item.estimated_tokens(&reference.frame.profile)).sum::<u64>();
        let added_from = entries.iter().rposition(|(node, _)| matches!(node.entry, TranscriptEntry::Compacted { .. })).map_or(0, |at| at + 1);
        let measured: Vec<Option<u64>> = entries.iter().enumerate()
            .map(|(at, (node, _))| match &node.entry {
                TranscriptEntry::Message(message) if at >= added_from && message.sender_id == agent => message.prompt_tokens,
                _ => None,
            })
            .collect();
        let mut positions = vec![size(&leading); entries.len() + 1];
        let first = measured.iter().position(Option::is_some);
        if let Some(first) = first {
            positions[first] = measured[first].unwrap_or_default();
            for at in (0..first).rev() {
                positions[at] = positions[at + 1].saturating_sub(size(&entries[at].1));
            }
        }
        for at in first.unwrap_or(0)..entries.len() {
            if let Some(before) = measured[at] {
                positions[at] = before;
            }
            positions[at + 1] = positions[at] + size(&entries[at].1);
        }
        Ok(Some((positions, first.is_some())))
    }

    /// What the model of `agent` is given of the transcript `reference`
    /// names, in turn `turn`, entry by entry, with the prompt parts where
    /// they were placed - before `ModelProfile::place_system_messages`.
    /// `None` as for `resolve`, and if a part's entry is not on the path.
    fn given_path(&self, reference: &ContextRef, turn: TurnId, agent: ActorId) -> Result<Option<GivenPath<'a>>, PromptError> {
        let prefix = match reference.transcript.as_ref() {
            Some(digest) => match self.transcript.prefix(digest) {
                Some(prefix) => prefix,
                None => return Ok(None),
            },
            None => Vec::new(),
        };
        let path: Vec<_> = as_they_stand(prefix).collect();
        // The turns given up because the channel was lowered below them:
        // what they read may not flow into it, and nobody released it (issue
        // 141), so the model is given which tools they called and nothing
        // else (issue 258).
        let lowered: HashMap<TurnId, &Classification> = path.iter()
            .filter_map(|(node, _)| match &node.entry {
                TranscriptEntry::TurnGivenUp { turn_id, reason: GivenUpReason::Lowered { channel }, .. } => Some((*turn_id, channel)),
                _ => None,
            })
            .collect();
        let profile = &reference.frame.profile;
        // As the context was recorded: in the variant its model was given.
        let texts = profile.texts(self.catalogue);
        // The parts after the transcript entry `after` - `None`: before
        // everything.
        let after_entry = |after: Option<Digest>| -> Option<Vec<ContextItem>> {
            reference.frame.prompts.iter()
                .filter(|p| p.after == after)
                .map(|p| Some(ContextItem::Prompt { digest: p.text, role: p.role, text: self.prompts.get(&p.text)?.clone(), classification: p.classification.clone() }))
                .collect()
        };
        let Some(leading) = after_entry(None) else { return Ok(None) };
        let mut placed = leading.len();
        let mut entries = Vec::with_capacity(path.len());
        for (node, message) in path {
            let in_turn = message.as_ref().is_some_and(|m| m.turn_id == Some(turn));
            let message = message.and_then(|m| match m.turn_id.and_then(|t| lowered.get(&t)) {
                Some(channel) => m.names_only(channel),
                None => Some(m),
            });
            let mut given = Vec::new();
            // The end of a turn is no message; one given up is told where it
            // stands, and so is who joined or left. A part may have been
            // placed after any of them.
            if let Some(message) = message {
                given.push(ContextItem::Conversation { message: profile.given(message, agent, in_turn, reference.frame.time_zone) });
            }
            if let TranscriptEntry::TurnGivenUp { turn_id, reason, at } = &node.entry {
                given.push(ContextItem::Conversation { message: profile.given(given_up_notice(&texts, *turn_id, reason, *at)?, agent, false, reference.frame.time_zone) });
            }
            if let Some(notice) = participant_notice(&texts, &node.entry, &node.digest, agent, profile.sender_style)? {
                given.push(ContextItem::Conversation { message: profile.given(notice, agent, false, reference.frame.time_zone) });
            }
            let Some(parts) = after_entry(Some(node.digest)) else { return Ok(None) };
            placed += parts.len();
            given.extend(parts);
            entries.push((node, given));
        }
        // Every part was placed, none after what is not there.
        Ok((placed == reference.frame.prompts.len()).then_some((leading, entries)))
    }
}

impl ContextItem {
    /// About how many tokens the model of `profile` is given of the item
    /// (issue 284), as `Message::estimated_tokens` estimates, and an image it
    /// is given along as `IMAGE_TOKENS` (issue 282).
    pub fn estimated_tokens(&self, profile: &ModelProfile) -> u64 {
        match self {
            ContextItem::Conversation { message } => {
                let images = message.attachments.iter().filter(|attachment| profile.gives_along(attachment)).count();
                message.estimated_tokens().saturating_add(IMAGE_TOKENS.saturating_mul(u64::try_from(images).unwrap_or(u64::MAX)))
            }
            ContextItem::Prompt { text, .. } => estimated_tokens(text.len()),
        }
    }
}

/// About how many tokens an image counts in a context (issue 282): what Ollama counts one as where
/// it truncates; an encoder of its own counts it otherwise.
const IMAGE_TOKENS: u64 = 768;

/// `text` as the value of an attribute of a marker: what would end it or open an element escaped,
/// nothing more - escaping more hides typos in code from the model (RESULTS.md, 7).
fn attribute(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// What the model is told of the turn `turn`, given up at `at` for `reason`
/// (issue 258), where its end stands: why, and that what its tools did stays
/// done, worded by `texts` (`core.given_up`). A fixed text of the
/// runtime's, which says nothing of the conversation; its id is the turn's,
/// so that it is the same each time it is resolved.
fn given_up_notice(texts: &PromptTexts, turn: TurnId, reason: &GivenUpReason, at: DateTime<Utc>) -> Result<Message, PromptError> {
    let cause = texts.render(&reason.cause_key(), ())?;
    let lowered = matches!(reason, GivenUpReason::Lowered { .. });
    Ok(Message {
        id: MessageId(turn.0),
        timestamp: at,
        role: MessageRole::System,
        content: Some(texts.render(&format!("{CORE}.given_up"), context! { cause, lowered })?),
        internal: true,
        turn_id: Some(turn),
        ..Message::new(Classification::PUBLIC)
    })
}

/// What the model of `agent` is told of `entry`, whose digest is `digest`,
/// if someone joined or left the channel (issue 408): who, and who made
/// them, named in `style` as the senders of messages are - the agent itself
/// as "you" -, worded by `texts` (`core.participant_joined`, `…_left`). A
/// fixed text of the runtime's, marked as the channel's notice, so that it
/// reads as no participant's, given as a system message where the model
/// keeps one (`ModelProfile::place_system_messages`); its id is the
/// entry's, so that it is the same each time it is resolved. `None` for any
/// other entry.
fn participant_notice(texts: &PromptTexts, entry: &TranscriptEntry, digest: &Digest, agent: ActorId, style: SenderStyle) -> Result<Option<Message>, PromptError> {
    let (key, participant, name, by, by_name, at) = match entry {
        TranscriptEntry::ParticipantJoined { participant, name, by, by_name, at } => ("participant_joined", participant, name, by, by_name, at),
        TranscriptEntry::ParticipantLeft { participant, name, by, by_name, at } => ("participant_left", participant, name, by, by_name, at),
        _ => return Ok(None),
    };
    let text = texts.render(&format!("{CORE}.{key}"), context! {
        participant => style.of(name),
        you => *participant == agent,
        by => style.of(by_name),
        by_you => *by == agent,
        by_self => by == participant,
        by_system => *by == ActorId::System,
    })?;
    Ok(Some(Message {
        id: MessageId(digest.as_uuid()),
        timestamp: *at,
        role: MessageRole::System,
        content: Some(text),
        sender_id: ActorId::System,
        ..Message::new(Classification::PUBLIC)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_catalogue::{core, CORE_PROMPTS};
    use chrono::{TimeZone as _, Utc};
    use crate::agent::AgentId;
    use crate::digest::prompt;
    use crate::message::{MessageId, TokenUsage};
    use crate::turn::TurnState;
    use crate::digest;
    use crate::tool::Tool;
    use crate::user::UserId;
    use crate::work::Work;
    use std::fs;

    fn text(content: &str) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
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

    /// Issue 282: every message with attachments carries a note on them, a marker in the form of
    /// the sender's, each with its number if its transcript gave it one, and whether the model is
    /// given it along - an image, to a model that sees.
    #[test]
    fn the_attachments_of_a_message_are_noted_as_the_models_rule_says() {
        use crate::content::testing::make_content_ref;
        let agent = ActorId::Agent(AgentId::new());
        let alice = ActorId::User(UserId::new());
        let image = ContentRef { number: Some(3), size: 2_300_000, ..make_content_ref("chart \"q3\".png", b"x") };
        let report = ContentRef { media_type: "application/pdf".to_string(), size: 512, ..make_content_ref("report.pdf", b"y") };
        let attached = Message { sender_id: alice, sender_name: "Alice".to_string(), role: MessageRole::User, attachments: vec![image.clone(), report.clone()], ..text("look") };
        let given = |profile: ModelProfile, message: Message| profile.given(message, agent, false, Default::default()).content.unwrap_or_default();

        let xml = ModelProfile { sender_format: SenderFormat::Xml, images: true, ..ModelProfile::default() };
        assert_eq!(given(xml, attached.clone()), "<message from=\"@alice\">look\n<attachment ref=\"a3\" name=\"chart &quot;q3&quot;.png\" type=\"image/png\" size=\"2.3 MB\" shown=\"yes\"/>\n<attachment name=\"report.pdf\" type=\"application/pdf\" size=\"512 B\" shown=\"no\"/></message>", "inside the marker, an image given along to a model that sees");
        let blind = ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() };
        assert_eq!(given(blind, attached.clone()), "@alice: look\n[attachment a3: chart \"q3\".png, image/png, 2.3 MB, not shown]\n[attachment report.pdf, application/pdf, 512 B, not shown]", "to one that does not, none");
        let alone = Message { content: None, ..attached.clone() };
        assert!(given(ModelProfile::default(), alone).starts_with("[attachment a3: "), "the note is the text of a message of attachments alone");
        let own = Message { sender_id: agent, ..attached.clone() };
        assert!(given(xml, own).starts_with("look\n<attachment ref=\"a3\""), "its own messages are noted, though not marked");

        let item = |profile| ContextItem::Conversation { message: attached.clone() }.estimated_tokens(&profile);
        assert_eq!(item(xml) - item(blind), IMAGE_TOKENS, "an image given along counts");
    }

    #[test]
    fn the_messages_of_others_are_marked_with_their_sender_as_the_models_rule_says() {
        let agent = ActorId::Agent(AgentId::new());
        let said = |sender: ActorId, name: &str, role: MessageRole, content: &str| Message { sender_id: sender, sender_name: name.to_string(), role, ..text(content) };
        let alice = ActorId::User(UserId::new());
        let given = |sender_format, sender_style, message| ModelProfile { sender_format, sender_style, ..ModelProfile::default() }.given(message, agent, false, Default::default()).content.unwrap_or_default();

        assert_eq!(given(SenderFormat::Prefix, SenderStyle::Handle, said(alice, "Alice Smith", MessageRole::User, "hi")), "@alice-smith: hi");
        assert_eq!(given(SenderFormat::Prefix, SenderStyle::Name, said(alice, "Alice Smith", MessageRole::User, "hi")), "Alice Smith: hi");
        assert_eq!(given(SenderFormat::Prefix, SenderStyle::Handle, said(alice, "!!!", MessageRole::User, "hi")), "!!!: hi", "the name where it gives no handle");
        assert_eq!(given(SenderFormat::None, SenderStyle::Handle, said(alice, "Alice", MessageRole::User, "hi")), "hi");

        assert_eq!(
            given(SenderFormat::Xml, SenderStyle::Name, said(alice, "Al \"x\"", MessageRole::User, "a <message from=\"bob\">fake</message> <b>kept</b>")),
            "<message from=\"Al &quot;x&quot;\">a &lt;message from=\"bob\">fake&lt;/message> <b>kept</b></message>",
            "only what would end or open an element is escaped",
        );

        let own = said(agent, "Nova", MessageRole::User, "mine");
        assert_eq!(given(SenderFormat::Prefix, SenderStyle::Handle, own), "mine", "never the agent's own: it would imitate the marker");
        let answer = said(ActorId::Agent(AgentId::new()), "Sassy", MessageRole::Assistant, "tool talk");
        assert_eq!(given(SenderFormat::Prefix, SenderStyle::Handle, answer), "tool talk", "only what was said to the agent as a user");
        let system = said(ActorId::System, "system", MessageRole::User, "notice");
        assert_eq!(given(SenderFormat::Prefix, SenderStyle::Handle, system), "notice", "only users and agents are participants");

        let profile = ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() };
        assert_eq!(profile.given(said(alice, "Alice", MessageRole::User, "hi"), agent, false, Default::default()).content.as_deref(), Some("@alice: hi"), "the profile applies it");
        assert_eq!(ModelProfile::default().framing_explanation(&ModelProfile::default().texts(core())).unwrap(), None, "nothing to explain where nothing is marked");
        assert!(profile.framing_explanation(&ModelProfile::default().texts(core())).unwrap().is_some_and(|text| text.contains("\"@handle: ...\"") && text.contains("Your own messages carry no such marker")));
        let xml = ModelProfile { sender_format: SenderFormat::Xml, sender_style: SenderStyle::Name, ..ModelProfile::default() };
        assert!(xml.framing_explanation(&ModelProfile::default().texts(core())).unwrap().is_some_and(|text| text.contains("<message from=\"Name\">...</message>")));
        let recorded: ModelProfile = serde_json::from_value(serde_json::json!({ "reasoning_history": "never", "system_messages": "anywhere", "message_times": false })).unwrap();
        assert_eq!(recorded.sender_format, SenderFormat::None, "a context recorded before was given without markers");
    }

    #[test]
    fn the_messages_of_others_say_when_they_were_written_where_the_profile_says_so() {
        let agent = ActorId::Agent(AgentId::new());
        let alice = ActorId::User(UserId::new());
        let at = Utc.with_ymd_and_hms(2026, 10, 18, 12, 32, 0).unwrap();
        let said = |sender: ActorId, content: &str| Message { sender_id: sender, sender_name: "Alice".to_string(), role: MessageRole::User, timestamp: at, ..text(content) };
        let berlin = TimeZone::new("Europe/Berlin").unwrap();
        let given = |sender_format, message| ModelProfile { sender_format, message_times: true, ..ModelProfile::default() }.given(message, agent, false, berlin).content.unwrap_or_default();

        assert_eq!(given(SenderFormat::Prefix, said(alice, "hi")), "@alice (Sun 18 Oct 2026, 14:32): hi", "in the time zone given");
        assert_eq!(given(SenderFormat::Xml, said(alice, "hi")), "<message from=\"@alice\" at=\"Sun 18 Oct 2026, 14:32\">hi</message>");
        assert_eq!(given(SenderFormat::None, said(alice, "hi")), "hi", "nothing where senders are not marked");
        assert_eq!(given(SenderFormat::Xml, said(agent, "mine")), "mine", "never on the agent's own: it would write times of its own");
        let without = ModelProfile { sender_format: SenderFormat::Xml, ..ModelProfile::default() };
        assert_eq!(without.given(said(alice, "hi"), agent, false, berlin).content.as_deref(), Some("<message from=\"@alice\">hi</message>"), "without the switch");

        let explained = |sender_format| ModelProfile { sender_format, message_times: true, ..ModelProfile::default() }.framing_explanation(&ModelProfile::default().texts(core())).unwrap().unwrap_or_default();
        assert!(explained(SenderFormat::Xml).contains("<message from=\"@handle\" at=\"Sun 18 Oct 2026, 14:32\">...</message>"), "{}", explained(SenderFormat::Xml));
        assert!(explained(SenderFormat::Prefix).contains("\"@handle (Sun 18 Oct 2026, 14:32): ...\""), "{}", explained(SenderFormat::Prefix));
        assert_eq!(TIME_EXAMPLE, berlin.format(at), "the example is written as the times are");

        assert!(serde_json::from_value::<ModelProfile>(serde_json::json!({ "reasoning_history": "never", "system_messages": "anywhere" })).is_err(),
            "a context recorded without saying whether it gave times is not read: it is to be deleted (issue 424)");
    }

    #[test]
    fn a_context_is_resolved_with_the_senders_marked_as_its_profile_says() {
        let agent = ActorId::Agent(AgentId::new());
        let alice = Message { sender_id: ActorId::User(UserId::new()), sender_name: "Alice".to_string(), role: MessageRole::User, ..text("question") };
        let mut state = AgentChannelState::default();
        state.transcript.push(alice.clone());
        state.transcript.push(Message { sender_id: agent, role: MessageRole::Assistant, ..text("answer") });
        let reference = |sender_format| ContextRef { transcript: state.transcript.digest(), frame: FrameRef { profile: ModelProfile { sender_format, ..ModelProfile::default() }, ..FrameRef::default() } };

        let resolver = ContextResolver::of(&state, core());
        let seen = resolver.resolve(&reference(SenderFormat::Prefix), TurnId::new(), agent).unwrap().expect("resolved");
        assert_eq!(contents(&seen.items), ["@alice: question", "answer"], "what the model is given, and the context panel shows");
        assert_eq!(state.transcript.messages()[0].content, alice.content, "the transcript keeps it as received");
        let seen = resolver.resolve(&reference(SenderFormat::None), TurnId::new(), agent).unwrap().expect("resolved");
        assert_eq!(contents(&seen.items), ["question", "answer"]);
    }

    #[test]
    fn who_joined_or_left_is_told_where_it_happened_as_the_channel_s_notice() {
        let agent = ActorId::Agent(AgentId::new());
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let at = Utc::now();
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { sender_id: alice, sender_name: "Alice".to_string(), role: MessageRole::User, ..text("hello") });
        let joined = |participant: ActorId, name: &str, by: ActorId, by_name: &str| TranscriptEntry::ParticipantJoined { participant, name: name.to_string(), by, by_name: by_name.to_string(), at };
        let left = |participant: ActorId, name: &str, by: ActorId, by_name: &str| TranscriptEntry::ParticipantLeft { participant, name: name.to_string(), by, by_name: by_name.to_string(), at };
        state.transcript.record(joined(bob, "Bob", alice, "Alice"));
        state.transcript.record(joined(agent, "Helper", alice, "Alice"));
        state.transcript.record(joined(bob, "Bob", bob, "Bob"));
        state.transcript.record(joined(agent, "Helper", ActorId::System, "system"));
        state.transcript.record(left(bob, "Bob", alice, "Alice"));
        state.transcript.record(left(bob, "Bob", bob, "Bob"));
        state.transcript.record(left(bob, "Bob", ActorId::System, "system"));
        let reference = ContextRef { transcript: state.transcript.digest(), frame: FrameRef { profile: ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() }, ..FrameRef::default() } };

        let resolver = ContextResolver::of(&state, core());
        let seen = resolver.resolve(&reference, TurnId::new(), agent).unwrap().expect("resolved");
        assert_eq!(contents(&seen.items), [
            "@alice: hello",
            "Channel notice: @alice added @bob to the channel.",
            "Channel notice: @alice added you to the channel.",
            "Channel notice: @bob joined the channel.",
            "Channel notice: You were added to the channel.",
            "Channel notice: @alice removed @bob from the channel.",
            "Channel notice: @bob left the channel.",
            "Channel notice: @bob was removed from the channel.",
        ], "in the order it happened, unmarked by any participant");
        assert!(seen.items[1..].iter().all(|item| item.role() == MessageRole::User), "not as a system message, which the model would move (issue 49)");
        let again = resolver.resolve(&reference, TurnId::new(), agent).unwrap().expect("resolved");
        assert_eq!(ids(&seen.items), ids(&again.items), "the same each time it is resolved");
        let told: Vec<_> = reference.frame.profile.given_messages(&ModelProfile::default().texts(core()), &state.transcript, agent, Default::default()).unwrap().into_iter().filter_map(|m| m.content).collect();
        assert_eq!(told, contents(&seen.items), "what a remote agent is given, too");
    }

    /// The ids of the messages among `items`.
    fn ids(items: &[ContextItem]) -> Vec<MessageId> {
        items.iter().filter_map(|item| match item {
            ContextItem::Conversation { message } => Some(message.id),
            ContextItem::Prompt { .. } => None,
        }).collect()
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
            frame: FrameRef {
                prompts: vec![
                    PromptRef { text: prompt("system"), role: MessageRole::System, after: None, classification: None },
                    PromptRef { text: prompt("notes"), role: MessageRole::User, after: before_question, classification: None },
                ],
                tools: None,
                knowledge: Vec::new(),
                profile: ModelProfile::default(), time_zone: Default::default(),
            },
        };

        let resolver = ContextResolver::of(&state, core());
        let turn = TurnId::new();
        let seen = resolver.resolve(&reference, turn, ActorId::None).unwrap().expect("everything it names is here");
        assert_eq!(contents(&seen.items), ["prompt:system", "earlier", "prompt:notes", "question"]);

        let mut elsewhere = reference.clone();
        elsewhere.frame.prompts[1].after = Some(prompt("not an entry"));
        assert!(resolver.resolve(&elsewhere, turn, ActorId::None).unwrap().is_none(), "a part after an entry that is not in the prefix");
        let mut forgotten = reference;
        forgotten.frame.prompts[0].text = prompt("never recorded");
        assert!(resolver.resolve(&forgotten, turn, ActorId::None).unwrap().is_none(), "a part whose text was not kept");
    }

    #[test]
    fn the_context_is_measured_by_the_calls_made_on_it_and_estimated_as_the_model_is_given_it() {
        let agent = ActorId::Agent(AgentId::new());
        let measure = |state: &AgentChannelState| {
            let turn = state.turn.as_ref().unwrap();
            ContextResolver::of(state, core()).token_positions(&turn.work.context, turn.work.turn_id, agent).unwrap().unwrap()
        };
        // Its reasoning: 100 tokens, as many as it produced in all.
        let call = |prompt: u64, content: &str| Message { sender_id: agent, role: MessageRole::Assistant, prompt_tokens: Some(prompt), completion_tokens: Some(101), reasoning: Some("r".repeat(400)), ..text(content) };
        let mut state = AgentChannelState::default();
        state.transcript.push(text("hello"));
        begin(&mut state, ModelProfile { reasoning_history: ReasoningHistory::Never, ..ModelProfile::default() });
        assert_eq!(measure(&state), (vec![0, 2], false), "nothing measured: \"hello\" estimated");

        let first = call(100, "call");
        state.said(first.clone());
        state.said(Message { sender_id: agent, role: MessageRole::Tool, ..text(&"x".repeat(40)) });
        assert_eq!(measure(&state), (vec![98, 100, 101, 111], true),
            "before the call as it measured, back from it and on from it estimated - the call as the model is given it, without its reasoning");
        state.turn.as_mut().unwrap().work.context.frame.profile.reasoning_history = ReasoningHistory::CurrentTurn;
        assert_eq!(measure(&state).0[2], 201, "with it where it is given back");

        let kept_from = state.transcript.entries().find(|n| n.entry.message().is_some_and(|m| m.id == first.id)).map(|n| n.digest);
        let summary = Message { sender_id: agent, prompt_tokens: Some(500), completion_tokens: Some(8), ..text("summary") };
        state.compact(agent, vec![(summary, None)], kept_from, core()).expect("compacted");
        assert_eq!(measure(&state), (vec![0, 2, 103, 113, 113], false), "neither the summary's call nor the copy measure the new branch, and where the compaction's part ends gives nothing");

        state.said(call(60, "done"));
        let (positions, measured) = measure(&state);
        assert!(measured, "a call on the new branch measures it");
        assert_eq!(positions[4..], [60, 161], "before it as it measured, then it as the model is given it");

        let mut others = AgentChannelState::default();
        others.transcript.push(text("hello"));
        begin(&mut others, ModelProfile::default());
        others.transcript.push(Message { sender_id: ActorId::Agent(AgentId::new()), ..call(70, "theirs") });
        others.turn.as_mut().unwrap().work.context.transcript = others.transcript.digest();
        assert!(!measure(&others).1, "a call of another agent's model does not measure this one's context");
    }

    /// A turn of `state` begun on its transcript as it stands.
    fn begin(state: &mut AgentChannelState, profile: ModelProfile) {
        let context = ContextRef { transcript: state.transcript.digest(), frame: FrameRef { profile, ..FrameRef::default() } };
        let work = Work { turn_id: TurnId::new(), context, ..Default::default() };
        state.turn = Some(TurnState { work, ..TurnState::default() });
    }

    /// A prompt part `text` in `role` after the end of `state`'s turn so
    /// far, its text kept.
    fn place(state: &mut AgentChannelState, role: MessageRole, text: &str) {
        state.prompts.keep(prompt(text), || text.to_string());
        let turn = state.turn.as_mut().unwrap();
        turn.work.context.frame.prompts.push(PromptRef { text: prompt(text), role, after: turn.work.context.transcript, classification: None });
    }

    #[test]
    fn a_turn_under_way_is_resolved_from_the_transcript_it_says_into() {
        let agent = ActorId::Agent(AgentId::new());
        let own = |content: &str| Message { sender_id: agent, reasoning: Some(format!("why {content}")), ..text(content) };
        let mut state = AgentChannelState::default();
        state.transcript.push(own("earlier"));
        state.transcript.push(text("question"));

        begin(&mut state, ModelProfile::default());
        // Instructions of the runtime among the turn's messages: one before
        // them, one after the first.
        place(&mut state, MessageRole::User, "step 1");
        let first = state.said(own("first"));
        place(&mut state, MessageRole::User, "step 2");
        let second = state.said(own("second"));

        let (next, turn_id) = state.turn.as_ref().map(|t| (t.work.context.clone(), t.work.turn_id)).unwrap();
        let current = ContextResolver::of(&state, core()).resolve(&next, turn_id, agent).unwrap().expect("the transcript holds the turn's messages");
        assert_eq!(contents(&current.items), ["earlier", "question", "prompt:step 1", "first", "prompt:step 2", "second"]);
        let reasoning = current.items.iter().map(|item| matches!(item, ContextItem::Conversation { message } if message.reasoning.is_some())).collect::<Vec<_>>();
        assert_eq!(reasoning, [false, false, false, true, false, true], "the turn's own reasoning, not an earlier turn's");

        // What each message was produced from, sent with it as it was said
        // (issues 260, 417), resolves to what the model was given for it -
        // while the turn is under way and once it ended.
        let seen = |state: &AgentChannelState, message: &Message| {
            let context = state.context_of(message).expect("sent with what it was produced from");
            contents(&ContextResolver::of(state, core()).resolve(&context, turn_id, agent).unwrap().expect("everything it names is here").items)
        };
        assert_eq!(seen(&state, &second), ["earlier", "question", "prompt:step 1", "first", "prompt:step 2"]);
        assert_eq!(state.transcript.digest(), next.transcript, "the turn's context names the transcript's head");
        state.turn = None;
        assert_eq!(seen(&state, &first), ["earlier", "question", "prompt:step 1"]);
        assert_eq!(seen(&state, &second), ["earlier", "question", "prompt:step 1", "first", "prompt:step 2"]);
    }

    #[test]
    fn usage_sums_the_agent_s_own_calls_on_every_branch_and_in_its_turn() {
        use crate::turn::GivenUpReason;
        let agent = ActorId::Agent(AgentId::new());
        let call = |sender: ActorId, prompt: u64, completion: u64, size: Option<u64>| Message {
            sender_id: sender,
            prompt_tokens: Some(prompt),
            completion_tokens: Some(completion),
            context_size: size,
            ..text("answer")
        };
        let mut state = AgentChannelState::default();
        assert_eq!(state.used, None, "no call yet");

        begin(&mut state, ModelProfile::default());
        state.said(call(agent, 100, 10, Some(4096)));
        // Heard: it carries its sender's tokens.
        let heard = call(ActorId::Agent(AgentId::new()), 5000, 500, Some(8192));
        state.perceive(vec![heard]);
        let before_given = state.transcript.digest();
        state.said(call(agent, 200, 20, Some(4096)));
        state.transcript.record(TranscriptEntry::TurnGivenUp { turn_id: TurnId::new(), reason: GivenUpReason::Left, at: Utc::now() });
        // A branch no longer active - as an external agent's earlier calls
        // are (`start_anew`) - counts as well.
        state.transcript.move_head(before_given.as_ref());
        begin(&mut state, ModelProfile::default());
        state.said(call(agent, 300, 30, None));

        assert_eq!(state.used, Some(TokenUsage {
            calls: 3,
            prompt_tokens: 600,
            completion_tokens: 60,
            context_size: None,
            context_tokens: 330,
        }), "not the other agent's call; the turn given up and the one under way count, the context as the last call measured it, its answer included");
    }

    /// Issue 57: a context is worded in the variant of the texts it was
    /// recorded with - for the next call of the model and for what it saw,
    /// asked afterwards, alike.
    #[test]
    fn a_context_is_worded_in_the_variant_it_was_recorded_with() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("core.yaml"), "given_up_cause:\n  left: you walked out\n").unwrap();
        let mut catalogue = PromptCatalogue::new();
        catalogue.add(CORE, CORE_PROMPTS).unwrap();
        catalogue.add_variant("gemma4", dir.path()).unwrap();
        let catalogue = Arc::new(catalogue);
        let mut state = AgentChannelState::default();
        state.transcript.record(TranscriptEntry::TurnGivenUp { turn_id: TurnId::new(), reason: GivenUpReason::Left, at: Utc::now() });
        let worded = |profile| {
            let reference = ContextRef { transcript: state.transcript.digest(), frame: FrameRef { profile, ..FrameRef::default() } };
            contents(&ContextResolver::of(&state, &catalogue).resolve(&reference, TurnId::new(), ActorId::None).unwrap().expect("resolved").items)
        };
        let gemma = ModelProfile { prompt_texts: Some(PromptVariant::new("gemma4").unwrap()), ..ModelProfile::default() };
        assert!(worded(gemma)[0].contains("because you walked out."), "{:?}", worded(gemma));
        assert!(worded(ModelProfile::default())[0].contains("because you left the channel."), "the default where it was recorded without one");
    }

    #[test]
    fn the_tools_offered_are_resolved_from_their_set() {
        use crate::tool::ToolParameters;
        let mut state = AgentChannelState::default();
        let echo = Tool { name: "echo".to_string(), description: "echoes".to_string(), parameters: ToolParameters::object([("text", ToolParameters::string().described("what"))], ["text"]), requires_approval: true, visibility: Default::default(), classification: None, presentation: Default::default() };
        let offered = vec![ToolSet::new("echo_tool", "echoes", vec![echo])];
        let digest = digest::tool_sets(&offered);
        state.offered_tools.keep(digest, || offered.clone());

        let resolver = ContextResolver::of(&state, core());
        let with = |tools| ContextRef { frame: FrameRef { tools, ..FrameRef::default() }, ..ContextRef::default() };
        assert_eq!(resolver.resolve(&with(Some(digest)), TurnId::new(), ActorId::None).unwrap().expect("the set is kept").tool_sets, offered);
        assert!(resolver.resolve(&with(None), TurnId::new(), ActorId::None).unwrap().expect("no tools offered").tool_sets.is_empty());
        assert!(resolver.resolve(&with(Some(prompt("never recorded"))), TurnId::new(), ActorId::None).unwrap().is_none(), "a set that was not kept");
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
            state.turn.as_mut().unwrap().work.context.frame.prompts.push(PromptRef { text: prompt("system prompt"), role: MessageRole::System, after: None, classification: None });
            place(&mut state, MessageRole::System, "instruction");
            let next = state.turn.as_ref().unwrap().work.context.clone();
            ContextResolver::of(&state, core()).resolve(&next, TurnId::new(), ActorId::None).unwrap().expect("everything it names is here")
                .items.iter().map(ContextItem::role).collect::<Vec<_>>()
        };
        use MessageRole::{System, User};
        assert_eq!(roles(SystemMessages::OnlyAtStart), [System, User, User, User], "only the system prompt stays one");
        assert_eq!(roles(SystemMessages::Anywhere), [System, User, System, System]);
        assert_eq!(SystemMessages::default(), SystemMessages::OnlyAtStart);
    }

    #[test]
    fn a_context_is_resolved_with_the_messages_as_they_stood_when_it_was_taken() {
        let mut state = AgentChannelState::default();
        let question = text("question");
        let after_question = {
            state.transcript.push(question.clone());
            state.transcript.digest()
        };
        state.transcript.push(text("aside"));
        let then = ContextRef { transcript: state.transcript.digest(), ..ContextRef::default() };
        state.prompts.keep(prompt("notes"), || "notes".to_string());

        let at = Utc::now();
        state.transcript.modify(Message { content: Some("question, corrected".to_string()), timestamp: at, ..question.clone() });
        let aside = state.transcript.messages()[1].id;
        state.transcript.revoke(aside, at);
        let notes = PromptRef { text: prompt("notes"), role: MessageRole::User, after: after_question, classification: None };
        let now = ContextRef { transcript: state.transcript.digest(), frame: FrameRef { prompts: vec![notes], ..FrameRef::default() } };

        let resolver = ContextResolver::of(&state, core());
        assert_eq!(contents(&resolver.resolve(&then, TurnId::new(), ActorId::None).unwrap().expect("resolved").items), ["question", "aside"], "what the model saw then");
        assert_eq!(contents(&resolver.resolve(&now, TurnId::new(), ActorId::None).unwrap().expect("resolved").items), ["question, corrected", "prompt:notes"], "what stands now");
    }
}
