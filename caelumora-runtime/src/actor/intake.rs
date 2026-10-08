//! How an actor of an agent takes in what it is told (issues 196, 206, 222,
//! 234), the same for both kinds of agent - the `AgentActor` and the
//! `ExternalAgentActor`: what others said, as it passes the agent's perceive
//! processors, a message modified or taken back since, and what it missed
//! of its channel while it was not there.

use crate::message::{apply_stage_processors, PerceiveInProcessor};
use crate::component::ComponentImpl;
use crate::runtime::ActorRuntime;
use crate::transport::EventReceiver;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, ActorId, Envelope};
use caelumora_model::agent::AgentChannelState;
use caelumora_model::message::Message;
use caelumora_model::transcript::TranscriptEntry;
use caelumora_model::security::SecurityContext;
use tracing::{debug, trace, warn, Instrument};

/// Takes what others said, `heard`, into the `state` of an agent (issues
/// 196, 206, 224), the same for both kinds of agent and every way in: all
/// of it into its history, as it was said; what it hears of it -
/// `Message::as_heard` - as it passes its perceive `processors`, into its
/// transcript. A failing stage is logged and leaves the transcript as it
/// was: what was heard stays in the history, and is not offered again.
///
/// What is heard is never the agent's own: the channel delivers nothing
/// back to its sender (`ChannelLoop::route_event_impl`), every message an
/// envelope carries is its sender's (`OriginEventSender`), and what an
/// agent catches up with is rid of its own first (`catch_up`).
pub(super) async fn perceive(
    state: &mut AgentChannelState,
    heard: Vec<Message>,
    processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext,
) {
    if heard.is_empty() {
        return;
    }
    let offered = heard.iter().filter_map(Message::as_heard).collect();
    let perceived = match apply_stage_processors(processors, offered, security).await {
        Ok(perceived) => perceived,
        Err(e) => {
            warn!("The perceive stage failed: {:#}", e);
            Vec::new()
        }
    };
    state.perceive(heard, perceived);
}

/// Takes in that a message of another reads as `modified` from now on
/// (issue 234), the same for both kinds of agent - never its own, as
/// nothing heard is (`perceive`): as it was modified into its history, if
/// that holds the message; as the agent hears it - `Message::as_heard`,
/// through its perceive `processors`, as everything heard - into its
/// transcript, if the message is on its active path, as what its
/// processors made of it under the message's id. Where nothing is left
/// under it, or the stage fails, the message is taken back there: the
/// version given before does not stand any more. Returns the change the
/// transcript did not take, as the agent hears it - a modification of
/// which nothing passed as taken back - or `None` if it took it.
async fn take_in_modified(
    state: &mut AgentChannelState,
    modified: Message,
    processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext,
) -> Option<TranscriptEntry> {
    let (id, at) = (modified.id, modified.timestamp);
    let perceived = match modified.as_heard() {
        None => None,
        Some(heard) => match apply_stage_processors(processors, vec![heard], security).await {
            Ok(perceived) => perceived.into_iter().find(|m| m.id == modified.id),
            Err(e) => {
                warn!("The perceive stage failed on a modified message: {:#}", e);
                None
            }
        },
    };
    state.history.modify(modified);
    let taken = match &perceived {
        Some(perceived) => state.transcript.modify(perceived.clone()),
        None => state.transcript.revoke(id, at),
    };
    if taken {
        return None;
    }
    Some(match perceived {
        Some(message) => TranscriptEntry::Modified { message },
        None => TranscriptEntry::Revoked { message: id, at },
    })
}

/// Takes in what an agent was told while it could not listen (issue 222),
/// the same for both kinds of agent: `received`, and everything `rx` holds
/// already behind it, in order. What others said is perceived as it comes
/// (`perceive`, with the agent's `processors`), and so is a message
/// modified or taken back since (issue 234); a reclassification is recorded
/// as it is (issue 239), in the history and the transcript where they hold
/// the message, the transcript on its active path. Of the requests to respond the last is returned -
/// one answer to the conversation as it stands, in the authority of the
/// last who asked - and the others are dropped, as is
/// every other event but the requests to answer again (issue 234), which
/// are returned in order, each to be answered. A modification or revocation
/// is recorded in the history and in the transcript where they hold the
/// message - the transcript on its active path; one the transcript did not
/// take is handed to `elsewhere` where it came (issue 236) - so in order
/// with what was said - as the agent hears it, a modification of which
/// nothing passes as taken back: for an actor that gave its transcript
/// elsewhere to tell. Neither commits nor stores: when that happens is the
/// actor's.
pub(super) async fn take_in(
    state: &mut AgentChannelState,
    received: Envelope,
    rx: &mut Box<dyn EventReceiver>,
    processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext,
    elsewhere: fn(&mut AgentChannelState, TranscriptEntry),
) -> (Option<Envelope>, Vec<Envelope>) {
    let mut request = None;
    let mut retries = Vec::new();
    let mut next = Some(received);
    while let Some(event) = next {
        let span = crate::logging::envelope_span(&event.context, event.event.kind());
        let asked = async {
            debug!("Received {}", event.describe());
            match event.event {
                ActorEvent::CompleteMessageGenerated(message) => {
                    trace!("Heard {:?}", message.content);
                    perceive(state, vec![message], processors, security).await;
                    None
                }
                ActorEvent::MessageModified(modified) => {
                    if let Some(change) = take_in_modified(state, modified, processors, security).await {
                        elsewhere(state, change);
                    }
                    None
                }
                ActorEvent::MessageRevoked { message, at } => {
                    state.history.revoke(message, at);
                    if !state.transcript.revoke(message, at) {
                        elsewhere(state, TranscriptEntry::Revoked { message, at });
                    }
                    None
                }
                ActorEvent::MessageReclassified { message, classification } => {
                    state.history.reclassify(message, classification.clone());
                    state.transcript.reclassify(message, classification);
                    None
                }
                ActorEvent::ResponseRequested => Some(event),
                ActorEvent::RetryRequested { .. } => {
                    retries.push(event);
                    None
                }
                _ => None,
            }
        }.instrument(span).await;
        request = asked.or(request);
        next = rx.try_recv();
    }
    (request, retries)
}

/// Takes in what an actor of an agent missed of its channel as what it
/// hears (issues 190, 196, 206): of `visible`, what its history does not
/// have yet, through `perceive` - with its perceive `processors` - and
/// stores its `state` with it in `runtime`. The one way in that does not
/// pass the channel's routing, so the agent `own`'s own messages are left
/// out here: they are in its history already, unless its state was lost,
/// and then they are not what it heard.
pub(super) async fn catch_up(
    state: &mut AgentChannelState,
    visible: Vec<Message>,
    own: ActorId,
    processors: &[ComponentImpl<dyn PerceiveInProcessor>],
    security: &SecurityContext,
    runtime: &ActorRuntime,
) -> Result<()> {
    // As it would have heard them (`Message::is_heard`, issue 258): the
    // steps of a plan and tool messages are kept in the channel, not heard.
    let missed: Vec<Message> = state.history.missing_from(visible).into_iter().filter(|m| m.sender_id != own && m.is_heard()).collect();
    if missed.is_empty() {
        return Ok(());
    }
    perceive(state, missed, processors, security).await;
    runtime.store_channel_state(state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::testing::Without;
    use crate::transport::EventReceiver;
    use caelumora_model::component::ComponentFailPolicy;
    use caelumora_model::message::MessageId;
    use caelumora_model::confidentiality::Classification;
    use std::sync::Arc;

    fn said(text: &str) -> Message {
        Message { role: caelumora_model::message::MessageRole::User, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
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
        let mut rx: Box<dyn EventReceiver> = Box::new(crate::transport::InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let security = SecurityContext::default();

        let heard = |text: &str| envelope(ActorEvent::CompleteMessageGenerated(said(text)));
        let mut last = envelope(ActorEvent::ResponseRequested);
        last.security.caller_id = caelumora_model::actor::ActorId::System;
        for event in [envelope(ActorEvent::ResponseRequested), heard("two"), envelope(ActorEvent::ResponseStarted { turn_id: caelumora_model::work::TurnId::new() }), last, heard("three")] {
            tx.try_send(event).expect("room");
        }

        let (request, _) = take_in(&mut state, heard("one"), &mut rx, &[], &security, |_, _| {}).await;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three"], "all of it, in order, also what came after the last request");
        assert_eq!(request.map(|r| r.security.caller_id), Some(caelumora_model::actor::ActorId::System), "the last request");
        assert!(rx.try_recv().is_none(), "nothing left behind");

        let (request, _) = take_in(&mut state, heard("four"), &mut rx, &[], &security, |_, _| {}).await;
        assert!(request.is_none(), "heard, but nobody asked");
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three", "four"]);
    }

    #[tokio::test]
    async fn a_change_is_recorded_where_the_chains_hold_the_message_and_handed_over_where_the_transcript_does_not() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(crate::transport::InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let security = SecurityContext::default();
        let (given, kept) = (Message { id: MessageId::new(), ..said("given") }, Message { id: MessageId::new(), ..said("kept") });
        state.perceive(vec![given.clone()], vec![given.clone()]);
        state.transcript.start_anew();
        state.perceive(vec![kept.clone()], vec![kept.clone()]);
        let modified = |message: &Message, text: &str| envelope(ActorEvent::MessageModified(Message { content: Some(text.to_string()), ..message.clone() }));
        let heard = |text: &str| envelope(ActorEvent::CompleteMessageGenerated(Message { id: MessageId::new(), ..said(text) }));
        // Writes down what it is handed, where it is handed it.
        fn noted(state: &mut AgentChannelState, change: TranscriptEntry) {
            let text = match change {
                TranscriptEntry::Modified { message } => format!("modified: {}", message.content.unwrap_or_default()),
                TranscriptEntry::Revoked { .. } => "taken back".to_string(),
                _ => "something else".to_string(),
            };
            state.transcript.push(Message { id: MessageId::new(), ..said(&text) });
        }

        tx.try_send(modified(&given, "given, corrected")).expect("room");
        tx.try_send(heard("and then")).expect("room");
        tx.try_send(envelope(ActorEvent::MessageRevoked { message: given.id, at: chrono::Utc::now() })).expect("room");
        tx.try_send(envelope(ActorEvent::MessageRevoked { message: MessageId::new(), at: chrono::Utc::now() })).expect("room");
        take_in(&mut state, modified(&kept, "kept, corrected"), &mut rx, &[], &security, noted).await;

        assert_eq!(texts(&state.transcript.messages()), ["kept, corrected", "modified: given, corrected", "and then", "taken back", "taken back"],
            "on the active path the transcript takes it; off it, it is handed over where it came - also what the agent never held: whether that is anything to tell is the actor's to see");
        assert_eq!(state.history.message(kept.id).and_then(|m| m.content).as_deref(), Some("kept, corrected"), "the history holds both as they stand");
        assert!(state.history.message(given.id).is_some_and(|m| m.revoked.is_some()));

        // A modification of which nothing passes the processors takes the
        // message back in the transcript; the history keeps what was said.
        let stages: [ComponentImpl<dyn PerceiveInProcessor>; 1] = [ComponentImpl { id: "no-spam".to_string(), fail_policy: ComponentFailPolicy::FailClosed, facet: Arc::new(Without(Some("spam"))) }];
        take_in(&mut state, modified(&kept, "kept, spam"), &mut rx, &stages, &security, noted).await;
        assert_eq!(texts(&state.transcript.messages()), ["modified: given, corrected", "and then", "taken back", "taken back"], "no longer given, and nothing handed over");
        assert_eq!(state.history.message(kept.id).and_then(|m| m.content).as_deref(), Some("kept, spam"));

        // A reclassification is recorded where the chains hold the message,
        // and handed over nowhere: a remote agent knows no labels.
        let earlier = Message { id: MessageId::new(), ..said("earlier") };
        state.perceive(vec![earlier.clone()], vec![earlier.clone()]);
        state.transcript.start_anew();
        let (public, before) = (Classification::PUBLIC, state.transcript.all_entries().len());
        take_in(&mut state, envelope(ActorEvent::MessageReclassified { message: earlier.id, classification: public.clone() }), &mut rx, &[], &security, noted).await;
        assert_eq!(state.history.message(earlier.id).map(|m| m.classification), Some(public), "in the history");
        assert_eq!(state.transcript.all_entries().len(), before, "off the transcript's path: nothing recorded, nothing handed over");
    }
}
