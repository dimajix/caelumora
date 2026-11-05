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
//!
//! Its parts: what a context is made of and refers to (`item`), how an agent's model is to be given
//! it (`profile`), and how a reference is resolved into what the model is given (`resolver`).

mod item;
mod profile;
mod resolver;
#[cfg(test)]
mod testing;

pub use item::{
    ContextItem, ContextRef, FrameRef, KnowledgeRef, KnowledgeSource, Placement, PromptPart,
    PromptRef,
};
pub use profile::{
    message_times_by_default, ModelProfile, ReasoningHistory, SenderFormat, SenderStyle,
    SystemMessages,
};
pub use resolver::{ContextResolver, ResolvedContext};
