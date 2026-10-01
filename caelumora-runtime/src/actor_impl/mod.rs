pub mod agent;
pub mod user;
pub mod approval;

use crate::agent_turn::Turn;
use crate::component::ComponentImpl;
use crate::message::MessageProcessor;
use anyhow::Result;
use caelumora_model::actor::ActorEvent;
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::message::Message;
use std::mem;
use tracing::warn;

/// Passes `input` through `processors` in turn, each given what the one
/// before returned. A failing processor is skipped if it fails open, and
/// fails the whole stage if it fails closed.
async fn apply_stage_processors<T>(
    processors: &[ComponentImpl<T>],
    mut input: Vec<Message>,
    turn: &mut Turn<'_>,
) -> Result<Vec<Message>>
where
    T: MessageProcessor + ?Sized + Send + Sync,
{
    for stage_processor in processors {
        let current = mem::take(&mut input);
        match stage_processor
            .process_messages(current, turn)
            .await
        {
            Ok(output) => {
                input = output;
            }
            Err(e) => match stage_processor.fail_policy {
                ComponentFailPolicy::FailOpen => {
                    warn!(
                        "Component '{}' failed with fail-open policy: {}",
                        stage_processor.id, e
                    );
                }
                ComponentFailPolicy::FailClosed => return Err(e),
            },
        }
    }
    Ok(input)
}

/// The event that publishes `message`: internal messages (plan steps)
/// are intermediate, the others what the channel shows as the answer;
/// `partial` for a streamed chunk.
fn event_for(message: Message, partial: bool) -> ActorEvent {
    match (message.internal, partial) {
        (true, true) => ActorEvent::IntermediatePartialGenerated(message),
        (false, true) => ActorEvent::PartialMessageGenerated(message),
        (true, false) => ActorEvent::IntermediateMessageGenerated(message),
        (false, false) => ActorEvent::CompleteMessageGenerated(message),
    }
}
