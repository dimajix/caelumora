//! What a context is made of and what refers to it (issues 06, 47, 49): the items a model is given
//! (`ContextItem`), the prompt parts and the knowledge they come from (`PromptRef`, `PromptPart`,
//! `KnowledgeRef`, `KnowledgeSource`), and the references a turn keeps (`ContextRef`, `FrameRef`).

use super::ModelProfile;
use crate::agent::AgentId;
use crate::chain::Position;
use crate::channel::ChannelId;
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::message::{estimated_tokens, Message, MessageRole};
use crate::session::SessionId;
use crate::store::{EntityKind, Stored};
use crate::value::time_zone::TimeZone;
use serde::{Deserialize, Serialize};
use std::ops::Not;
use uuid::Uuid;

/// A prompt part an agent's model was given besides the conversation - the
/// system prompt, notes, ... (issue 47): the digest of its text, the role it
/// was given as, where it stood (`PromptAt`), and how what it holds besides
/// the knowledge it lists was classified (issue 168), `None` if that is not
/// known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRef {
    pub text: Digest,
    pub role: MessageRole,
    pub at: PromptAt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
}

/// Where a prompt part of a frame stands in what its model is given (issues
/// 47, 537).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAt {
    /// Before everything.
    Start,
    /// After the transcript entry with this digest.
    After(Digest),
    /// After everything a call of the model is given - the transcript it is
    /// given and every part placed after its entries -, so that it moves
    /// on with every call and is read last.
    End,
}

impl PromptAt {
    /// After the entry `entry` - before everything if there is none: where
    /// a part goes that follows what a transcript holds so far.
    pub fn after(entry: Option<Digest>) -> Self {
        entry.map_or(PromptAt::Start, PromptAt::After)
    }
}

/// An element of what an agent's model is given, in the shape it was given
/// (issue 119): a message of the conversation, or a prompt part where the
/// `ContextProvider` placed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a message is the usual element, and boxing it would cost an allocation per element"
)]
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

    /// About how many tokens the model of `profile` is given of the item (issue 284), as
    /// `Message::estimated_tokens` estimates, and an image it is given along - attached, or given
    /// or shown by a tool - as `IMAGE_TOKENS` (issues 282, 512).
    pub fn estimated_tokens(&self, profile: &ModelProfile) -> u64 {
        match self {
            ContextItem::Conversation { message } => {
                let images =
                    message.carried().filter(|content| profile.gives_along(content)).count();
                message.estimated_tokens().saturating_add(
                    IMAGE_TOKENS.saturating_mul(u64::try_from(images).unwrap_or(u64::MAX)),
                )
            }
            ContextItem::Prompt { text, .. } => estimated_tokens(text.len()),
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
    /// Whether it is the agent's private knowledge - its private notes (issue 51), its memory
    /// (issues 62, 518) -, which makes what the turn produces from then on private (issue 61).
    #[serde(default, skip_serializing_if = "Not::not")]
    pub private: bool,
}

/// What a [`KnowledgeRef`] read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// An entity of whatever kind (issue 323), as its kind records what was read of it: the
    /// files of a workspace (issue 251), read through the channel it is used in, a file or
    /// directory by its path as `place` (issue 538) - the workspace's top directory an empty one -,
    /// with the digest of a file's content as it was read - `None` for a listing or a search -; a
    /// skill (issue 22), loaded into the turn whole, without a place, with the digest of its body
    /// as it was loaded (the skills plugin's `skill::digest`); an agent's memory (issues 62, 518),
    /// read by that agent, with the digest of its chain, recorded once for every channel the
    /// entries recalled were written in, so that knowledge from another session can be told (issue
    /// 134).
    Entity {
        kind: EntityKind,
        id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<String>,
        digest: Option<Digest>,
    },
}

impl KnowledgeSource {
    /// That the entity of kind `T` with `id` was read as a whole, as `digest` says.
    pub fn entity<T: Stored>(id: T::Id, digest: Option<Digest>) -> Self {
        Self::Entity { kind: T::KIND, id: id.into(), place: None, digest }
    }

    /// That `place` within the entity of kind `T` with `id` was read, as `digest` says - a file
    /// of a workspace by its path (issue 538).
    pub fn within<T: Stored>(id: T::Id, place: String, digest: Option<Digest>) -> Self {
        Self::Entity { kind: T::KIND, id: id.into(), place: Some(place), digest }
    }

    /// The id of the entity of kind `T` this is the knowledge of, and the
    /// digest of what was read; `None` for knowledge of anything else.
    pub fn about<T: Stored>(&self) -> Option<(T::Id, Option<Digest>)> {
        match *self {
            Self::Entity { kind, id, digest, .. } if kind == T::KIND => {
                Some((T::Id::from(id), digest))
            }
            _ => None,
        }
    }

    /// The kind of the entity this is the knowledge of; `None` for knowledge of anything else.
    pub fn kind(&self) -> Option<EntityKind> {
        match self {
            Self::Entity { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// The digest of what the model was given whole of an entity, or what a tool wrote whole of
    /// it (issue 538) - a skill loaded, a file read or written -, which can be given again and a
    /// write be checked against; `None` for a listing or a search, a file offered by a link, and
    /// knowledge of anything else.
    pub fn read_whole(&self) -> Option<Digest> {
        match self {
            Self::Entity { digest, .. } => *digest,
            _ => None,
        }
    }

    /// Whether `self` and `other` are knowledge of the same, whatever was read of it when: the
    /// same place within the same entity, or the same entity read as a whole (issue 538) - what a
    /// compaction gives again once; knowledge of anything else where it is equal.
    pub fn of_the_same(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Entity { kind, id, place, .. },
                Self::Entity { kind: other_kind, id: other_id, place: other_place, .. },
            ) => kind == other_kind && id == other_id && place == other_place,
            _ => self == other,
        }
    }

    /// Where within its entity what was read lies (`within`); `None` for an entity read as a
    /// whole, and for knowledge of anything else.
    pub fn place(&self) -> Option<&str> {
        match self {
            Self::Entity { place, .. } => place.as_deref(),
            _ => None,
        }
    }
}

/// A prompt part as a `ContextProvider` places it: its text, where it goes,
/// and how what it holds besides knowledge is classified, `None` if that is
/// not known (issue 168).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPart {
    pub text: String,
    pub placement: Placement,
    pub classification: Option<Classification>,
}

/// Where a prompt part goes (issues 47, 537), and the component it came
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Into the frame, at `at`, placed anew in every turn, given in `role`.
    /// `component` is the one it came from, if from one alone (issue 214):
    /// what an external agent's actor tells the parts it gives apart by.
    Frame { at: PromptAt, role: MessageRole, component: Option<String> },
    /// Into the transcript as the part of `component`, where its text
    /// changed (`AgentChannelState::give_parts`), given as what is given to
    /// read, in no role of its own (issue 539).
    Given { component: String },
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

/// About how many tokens an image counts in a context (issue 282): what Ollama counts one as where
/// it truncates; an encoder of its own counts it otherwise.
const IMAGE_TOKENS: u64 = 768;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::testing::make_content_ref;
    use crate::content::ContentRef;
    use crate::context::testing::text;
    use crate::digest;
    use crate::workspace::{Workspace, WorkspaceId};

    /// Issue 538: what was read within an entity is known by its place, kept where there is one;
    /// an entity read as a whole is kept without, as before.
    #[test]
    fn a_place_within_an_entity_is_kept_where_there_is_one() {
        let id = WorkspaceId::new();
        let file = KnowledgeSource::within::<Workspace>(id, "docs/plan.md".to_string(), None);
        let whole = KnowledgeSource::entity::<Workspace>(id, None);
        assert_eq!((file.place(), whole.place()), (Some("docs/plan.md"), None));
        assert_eq!(file.about::<Workspace>(), whole.about::<Workspace>(), "of the same entity");
        let changed = KnowledgeSource::within::<Workspace>(
            id,
            "docs/plan.md".to_string(),
            Some(digest::prompt("v2")),
        );
        assert!(file.of_the_same(&changed), "the same place, whatever was read of it");
        assert!(!file.of_the_same(&whole), "not the whole entity");
        let kept = serde_json::to_value(&whole).unwrap();
        assert!(kept["Entity"].get("place").is_none(), "{kept}");
        let read: KnowledgeSource =
            serde_json::from_value(serde_json::to_value(&file).unwrap()).unwrap();
        assert_eq!(read, file);
    }

    /// Issues 282, 284: an image given along counts as `IMAGE_TOKENS` more, for a model that sees
    /// it, than for one that does not.
    #[test]
    fn an_image_given_along_counts_in_the_estimate() {
        let image = ContentRef { number: Some(3), ..make_content_ref("chart.png", b"x") };
        let attached = Message { attachments: vec![image], ..text("look") };
        let item = |profile| {
            ContextItem::Conversation { message: attached.clone() }.estimated_tokens(&profile)
        };
        let seeing = ModelProfile { images: true, ..ModelProfile::default() };
        assert_eq!(item(seeing) - item(ModelProfile::default()), IMAGE_TOKENS);
    }
}
