//! The facets of an actor's message stages: components that see the
//! messages an actor perceives (`PerceiveInProcessor`) or an agent emits
//! (`EmitOutProcessor`), and may drop, change or add messages (issue 196).
//! A processor records nothing: an actor keeps all it was offered in its
//! history, and what passes its processors - what it processes - in its
//! transcript.

use async_trait::async_trait;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;

#[async_trait]
pub trait MessageProcessor: Send + Sync + std::fmt::Debug {
    /// What is made of `input` - filtered, changed, added to - for the actor
    /// whose context is `security`: who perceives or emits it, and where.
    async fn process_messages(&self, input: Vec<Message>, security: &SecurityContext) -> anyhow::Result<Vec<Message>>;
}

pub trait PerceiveInProcessor: MessageProcessor {}
pub trait EmitOutProcessor: MessageProcessor {}
