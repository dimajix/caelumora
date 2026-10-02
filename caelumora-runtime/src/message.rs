//! The facets of an actor's message stages: components that see the
//! messages an actor perceives (`PerceiveInProcessor`) or an agent emits
//! (`EmitOutProcessor`), and may filter or change them (issue 196). What
//! passes is the actor's to record; a processor records nothing.

use async_trait::async_trait;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;

#[async_trait]
pub trait MessageProcessor: Send + Sync + std::fmt::Debug {
    /// What is left of `input` - filtered, changed - for the actor whose
    /// context is `security`: who perceives or emits it, and where.
    async fn process_messages(&self, input: Vec<Message>, security: &SecurityContext) -> anyhow::Result<Vec<Message>>;
}

pub trait PerceiveInProcessor: MessageProcessor {}
pub trait EmitOutProcessor: MessageProcessor {}
