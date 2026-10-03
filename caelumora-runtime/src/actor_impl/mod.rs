pub mod agent;
pub mod user;
pub mod approval;
pub mod external_agent;

use crate::component::ComponentImpl;
use crate::message::{MessageProcessor, PerceiveInProcessor};
use crate::runtime::ActorRuntime;
use anyhow::Result;
use caelumora_model::actor::ActorEvent;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use tracing::warn;

/// Passes `input` through `processors` in turn, each given what the one
/// before returned, for the actor whose context is `security`. A failing
/// processor is skipped if it fails open - the next one is given what it
/// was given - and fails the whole stage if it fails closed.
async fn apply_stage_processors<T>(
    processors: &[ComponentImpl<T>],
    mut input: Vec<Message>,
    security: &SecurityContext,
) -> Result<Vec<Message>>
where
    T: MessageProcessor + ?Sized + Send + Sync,
{
    for stage_processor in processors {
        let kept = matches!(stage_processor.fail_policy, ComponentFailPolicy::FailOpen).then(|| input.clone());
        match stage_processor.process_messages(input, security).await {
            Ok(output) => input = output,
            Err(e) => match kept {
                Some(kept) => {
                    warn!("Component '{}' failed with fail-open policy: {}", stage_processor.id, e);
                    input = kept;
                }
                None => return Err(e),
            },
        }
    }
    Ok(input)
}

/// Takes what others said, `heard`, into an agent's `state` (issues 196,
/// 206), the same for both kinds of agent: all of it into its history; what
/// of it the agent `takes` in, as it passes its perceive `processors`, into
/// its transcript. A failing stage is logged and leaves the transcript as
/// it was: what was heard stays in the history, and is not offered again.
async fn perceive(
    state: &mut AgentChannelState,
    heard: Vec<Message>,
    takes: impl Fn(&Message) -> bool,
    processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext,
) {
    if heard.is_empty() {
        return;
    }
    let offered = heard.iter().filter(|m| takes(m)).cloned().collect();
    let perceived = match apply_stage_processors(processors, offered, security).await {
        Ok(perceived) => perceived,
        Err(e) => {
            warn!("The perceive stage failed: {:#}", e);
            Vec::new()
        }
    };
    state.perceive(heard, perceived);
}

/// Takes in what an actor of an agent missed of its channel as what it
/// hears (issues 190, 196, 206): of `visible`, what its history does not
/// have yet, through `perceive` - what of it the agent `takes`, as it passes
/// its perceive `processors` - and stores its `state` with it in `runtime`.
async fn catch_up(
    state: &mut AgentChannelState,
    visible: Vec<Message>,
    takes: impl Fn(&Message) -> bool,
    processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext,
    runtime: &ActorRuntime,
) -> Result<()> {
    let missed = state.history.missing_from(visible);
    if missed.is_empty() {
        return Ok(());
    }
    perceive(state, missed, takes, processors, security).await;
    runtime.store_channel_state(state).await
}

/// What the channel's users are told of `error`, which ended an answer: its
/// message where it is one for them (`crate::error::user_message`), else
/// that the details are in the server log - which has them, and nothing
/// else about the server.
fn failure_message(error: &anyhow::Error) -> String {
    crate::error::user_message(error).unwrap_or_else(|| "internal error - the details are in the server log".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use caelumora_model::confidentiality::Classification;
    use std::sync::Arc;

    /// Keeps what does not contain `word`, or fails.
    #[derive(Debug)]
    struct Without(Option<&'static str>);

    #[async_trait]
    impl MessageProcessor for Without {
        async fn process_messages(&self, input: Vec<Message>, _security: &SecurityContext) -> Result<Vec<Message>> {
            let Some(word) = self.0 else { return Err(anyhow::anyhow!("broken")) };
            Ok(input.into_iter().filter(|m| !m.content.as_deref().unwrap_or_default().contains(word)).collect())
        }
    }

    fn stage(id: &str, processor: Without, fail_policy: ComponentFailPolicy) -> ComponentImpl<dyn MessageProcessor> {
        ComponentImpl { id: id.to_string(), fail_policy, facet: Arc::new(processor) }
    }

    fn said(text: &str) -> Message {
        Message { content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
    }

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages.iter().filter_map(|m| m.content.as_deref()).collect()
    }

    #[tokio::test]
    async fn each_processor_filters_what_the_one_before_left_and_a_broken_one_that_fails_open_is_skipped() {
        let security = SecurityContext::default();
        let input = vec![said("hello"), said("secret plan"), said("spam offer")];
        let stages = [
            stage("no-secrets", Without(Some("secret")), ComponentFailPolicy::FailClosed),
            stage("broken", Without(None), ComponentFailPolicy::FailOpen),
            stage("no-spam", Without(Some("spam")), ComponentFailPolicy::FailClosed),
        ];
        let output = apply_stage_processors(&stages, input.clone(), &security).await.expect("the broken one fails open");
        assert_eq!(texts(&output), vec!["hello"], "the next is given what the broken one was given");

        let stages = [stage("broken", Without(None), ComponentFailPolicy::FailClosed)];
        assert!(apply_stage_processors(&stages, input, &security).await.is_err(), "failing closed fails the stage");
    }
}
