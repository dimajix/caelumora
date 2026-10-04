pub mod agent;
pub mod user;
pub mod approval;
pub mod external_agent;

use crate::component::ComponentImpl;
use crate::message::{MessageProcessor, PerceiveInProcessor};
use crate::runtime::ActorRuntime;
use crate::transport::EventReceiver;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, Envelope};
use caelumora_model::agent::AgentChannelState;
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use tracing::{debug, trace, warn, Instrument};

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
        // What it was given, should it be left out.
        let kept = matches!(stage_processor.fail_policy, ComponentFailPolicy::FailOpen).then(|| input.clone());
        let output = stage_processor.process_messages(input, security).await;
        input = match stage_processor.apply_fail_policy(output)? {
            Some(output) => output,
            None => kept.unwrap_or_default(),
        };
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

/// Takes in what an agent was told while it could not listen (issue 222),
/// the same for both kinds of agent: `received`, and everything `rx` holds
/// already behind it, in order. What others said is perceived as it comes
/// (`perceive`, with `takes` and `processors`); of the requests to respond
/// the last is returned - one answer to the conversation as it stands, in
/// the authority of the last who asked - and the others are dropped, as is
/// every other event. Neither commits nor stores: when that happens is the
/// actor's.
async fn take_in(
    state: &mut AgentChannelState,
    received: Envelope,
    rx: &mut Box<dyn EventReceiver>,
    takes: impl Fn(&Message) -> bool,
    processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext,
) -> Option<Envelope> {
    let mut request = None;
    let mut next = Some(received);
    while let Some(event) = next {
        let span = crate::logging::envelope_span(&event.context, event.event.kind());
        let asked = async {
            debug!("Received {}", event.describe());
            match event.event {
                ActorEvent::FullResponseGenerated(messages) => {
                    trace!("Heard {:?}", messages.iter().map(|m| &m.content).collect::<Vec<_>>());
                    perceive(state, messages, &takes, processors, security).await;
                    None
                }
                ActorEvent::ResponseRequested => Some(event),
                _ => None,
            }
        }.instrument(span).await;
        request = asked.or(request);
        next = rx.try_recv();
    }
    request
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

/// What the channel's users are told when the work of the agent `name` -
/// a turn, or an external agent's answer or task - was given up because
/// the channel was lowered to `channel` since it began (issues 141, 212).
fn lowered_message(name: &str, channel: &caelumora_model::confidentiality::Classification) -> String {
    format!("The channel's classification was lowered to {channel} while {name} was answering; the answer was discarded.")
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

    fn envelope(event: ActorEvent) -> Envelope {
        Envelope::new(SecurityContext::default(), caelumora_model::actor::EventContext::default(), event)
    }

    #[tokio::test]
    async fn everything_queued_is_heard_in_order_and_only_the_last_request_is_answered() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(crate::transport_impl::memory::InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let security = SecurityContext::default();

        let heard = |text: &str| envelope(ActorEvent::FullResponseGenerated(vec![said(text)]));
        let mut last = envelope(ActorEvent::ResponseRequested);
        last.security.caller_id = caelumora_model::actor::ActorId::System;
        for event in [envelope(ActorEvent::ResponseRequested), heard("two"), envelope(ActorEvent::ResponseStarted), last, heard("three")] {
            tx.try_send(event).expect("room");
        }

        let request = take_in(&mut state, heard("one"), &mut rx, |_| true, &[], &security).await;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three"], "all of it, in order, also what came after the last request");
        assert_eq!(request.map(|r| r.security.caller_id), Some(caelumora_model::actor::ActorId::System), "the last request");
        assert!(rx.try_recv().is_none(), "nothing left behind");

        let request = take_in(&mut state, heard("four"), &mut rx, |_| true, &[], &security).await;
        assert!(request.is_none(), "heard, but nobody asked");
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three", "four"]);
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
