//! How an actor of an agent takes in what it is told (issues 196, 206, 222,
//! 234), the same for both kinds of agent - the `AgentActor` and the
//! `ExternalAgentActor`: what others said, as it passes the agent's perceive
//! processors, a message modified or taken back since, and what it missed
//! of its channel while it was not there. Its body's (`AgentBody`).

use super::body::AgentBody;
use crate::message::apply_stage_processors;
use caelumora_runtime::logging::envelope_span;
use caelumora_runtime::transport::EventReceiver;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, Envelope};
use caelumora_model::agent::AgentChannelState;
use caelumora_model::history::{fold, HistoryEntry};
use caelumora_model::message::Message;
use caelumora_model::transcript::TranscriptEntry;
use std::mem;
use tracing::{debug, trace, warn, Instrument};

impl AgentBody {
    /// Takes what others said, `heard`, into the agent's `state` (issues
    /// 196, 206, 224), the same for both kinds of agent and every way in: all
    /// of it into its history, as it was said; what it hears of it -
    /// `Message::as_heard` - as it passes its perceive processors, into its
    /// transcript. A failing stage is logged and leaves the transcript as it
    /// was: what was heard stays in the history, and is not offered again.
    ///
    /// What is heard is never the agent's own: the channel delivers nothing
    /// back to its sender (`ChannelLoop::route_event_impl`), every message an
    /// envelope carries is its sender's (`OriginEventSender`), and what an
    /// agent catches up with is rid of its own first (`catch_up`).
    async fn perceive(&self, state: &mut AgentChannelState, heard: Vec<Message>) {
        if heard.is_empty() {
            return;
        }
        let offered = heard.iter().filter_map(Message::as_heard).collect();
        let perceived = match apply_stage_processors(&self.perceive_in_processors, offered, &self.security_context()).await {
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
    /// through its perceive processors, as everything heard - into its
    /// transcript, if the message is on its active path, as what its
    /// processors made of it under the message's id. Where nothing is left
    /// under it, or the stage fails, the message is taken back there: the
    /// version given before does not stand any more. Returns the change the
    /// transcript did not take, as the agent hears it - a modification of
    /// which nothing passed as taken back - or `None` if it took it.
    async fn take_in_modified(&self, state: &mut AgentChannelState, modified: Message) -> Option<TranscriptEntry> {
        let (id, at) = (modified.id, modified.timestamp);
        let perceived = match modified.as_heard() {
            None => None,
            Some(heard) => match apply_stage_processors(&self.perceive_in_processors, vec![heard], &self.security_context()).await {
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
    /// (`perceive`), and so is a message
    /// modified or taken back since (issue 234); a reclassification is recorded
    /// as it is (issue 239), in the history and the transcript where they hold
    /// the message, the transcript on its active path, and so is who joined
    /// or left (issue 408). Of the requests to respond the last is returned -
    /// one answer to the conversation as it stands, in the authority of the
    /// last who asked - and the others are dropped, as is
    /// every other event but the requests to answer again (issue 234), which
    /// are returned in order, each to be answered. A modification or revocation
    /// is recorded in the history and in the transcript where they hold the
    /// message - the transcript on its active path; one the transcript did not
    /// take is handed over (`AgentBody::hand_over`) where it came (issue 236) - so in order
    /// with what was said - as the agent hears it, a modification of which
    /// nothing passes as taken back: for an actor that gave its transcript
    /// elsewhere to tell. A command it is asked to carry out (issue 269) ends
    /// the batch: it is returned, and what follows it stays in `rx`, so that the
    /// actor carries it out where it came - after what was said before it,
    /// before what was said after - and then takes in the rest. Neither commits
    /// nor stores: when that happens is the actor's.
    pub(super) async fn take_in(&self, state: &mut AgentChannelState, received: Envelope, rx: &mut Box<dyn EventReceiver>) -> TakenIn {
        let mut request = None;
        let mut retries = Vec::new();
        let mut next = Some(received);
        while let Some(event) = next {
            if matches!(event.event, ActorEvent::CommandRequested { .. }) {
                return TakenIn { request, retries, command: Some(event) };
            }
            let span = envelope_span(&event.context, event.event.kind());
            let asked = async {
                debug!("Received {}", event.describe());
                match event.event {
                    ActorEvent::CompleteMessageGenerated(message) => {
                        trace!("Heard {:?}", message.content);
                        self.perceive(state, vec![message]).await;
                        None
                    }
                    ActorEvent::MessageModified(modified) => {
                        if let Some(change) = self.take_in_modified(state, modified).await {
                            (self.hand_over)(&self.runtime, state, change).await;
                        }
                        None
                    }
                    ActorEvent::MessageRevoked { message, at } => {
                        state.history.revoke(message, at);
                        if !state.transcript.revoke(message, at) {
                            (self.hand_over)(&self.runtime, state, TranscriptEntry::Revoked { message, at }).await;
                        }
                        None
                    }
                    ActorEvent::MessageReclassified { message, classification } => {
                        state.history.reclassify(message, classification.clone());
                        state.transcript.reclassify(message, classification);
                        None
                    }
                    // Who joined or left (issue 408): noted where it came,
                    // asking nothing of the agent.
                    ActorEvent::ParticipantJoined { .. } | ActorEvent::ParticipantLeft { .. } => {
                        if let Some(entry) = event.announced() {
                            state.note_announced(entry);
                        }
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
        TakenIn { request, retries, command: None }
    }

    /// Takes in what an actor of an agent missed of its channel as what it
    /// hears (issues 190, 196, 206): the messages said in the channel from
    /// the position `from` on - where the part of the channel's history it
    /// may see begins - and after the last message its history has that the
    /// channel's has, as they stand, through `perceive`, and who joined or
    /// left meanwhile, where it happened (issue 408), and stores its state
    /// with them. Read from the store (issue 379): the agent's history from
    /// its end backwards until a message of it is found in the channel's,
    /// then the channel's entries after it. The one way in that does not
    /// pass the channel's routing, so the agent's own messages are left out
    /// here: they are in its history already, unless its state was lost, and
    /// then they are not what it heard. Older messages that a wider view
    /// grants on joining again would land after its own later answers; they
    /// are left out, and its history stays in the order things were said.
    /// Called once, before its actor runs.
    ///
    /// # Errors
    /// If the channel's history cannot be read, or its state not be stored.
    pub(super) async fn catch_up(&self, from: u64) -> Result<()> {
        let mut state = self.state.lock().await;
        let own = self.id();
        let mut after = from;
        for message in state.history.all_entries().rev().filter_map(|node| node.entry.said()) {
            if let Some(at) = self.runtime.channel_position_of(message.id).await? {
                after = after.max(at + 1);
                break;
            }
        }
        let entries = self.runtime.channel_entries(after).await?;
        // As it would have heard them (`Message::is_heard`, issue 258): the
        // steps of a plan and tool messages are kept in the channel, not
        // heard. Who joined or left is noted where it happened among them
        // (issue 408).
        let mut said = fold(&entries).into_iter().peekable();
        let mut heard = Vec::new();
        let mut missed = false;
        for node in &entries {
            if matches!(node.entry, HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. }) {
                self.perceive(&mut state, mem::take(&mut heard)).await;
                state.note_announced(node.entry.clone());
                missed = true;
            } else if let Some((_, message)) = said.next_if(|(said, _)| said.seq == node.seq) {
                if message.sender_id != own && message.is_heard() {
                    heard.push(message);
                    missed = true;
                }
            }
        }
        if !missed {
            return Ok(());
        }
        self.perceive(&mut state, heard).await;
        self.runtime.store_channel_state(&mut state).await
    }
}

/// What `take_in` found in what it took in: the last request to respond,
/// the requests to answer again in order, and the command that ended the
/// batch, if one did.
pub(super) struct TakenIn {
    pub(super) request: Option<Envelope>,
    pub(super) retries: Vec<Envelope>,
    pub(super) command: Option<Envelope>,
}

impl TakenIn {
    /// This batch followed by `next`, taken in after the command that ended
    /// this one was carried out: the later request to respond, the requests
    /// to answer again of both in order, and the command that ended `next`.
    pub(super) fn then(self, next: TakenIn) -> TakenIn {
        let mut retries = self.retries;
        retries.extend(next.retries);
        TakenIn { request: next.request.or(self.request), retries, command: next.command }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::history::testing::messages_of;
    use caelumora_runtime::component::ComponentImpl;
    use crate::message::testing::Without;
    use crate::message::PerceiveInProcessor;
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::security::SecurityContext;
    use caelumora_runtime::runtime::ActorRuntime;
    use caelumora_runtime::transport::{EventReceiver, InMemoryEventReceiver};
    use futures::future::BoxFuture;
    use std::future::ready;
    use caelumora_model::component::ComponentFailPolicy;
    use caelumora_model::message::{MessageId, MessageRole};
    use caelumora_model::confidentiality::Classification;
    use std::sync::Arc;
    use caelumora_model::user::UserId;
    use caelumora_model::work::TurnId;
    use chrono::Utc;

    fn said(text: &str) -> Message {
        Message { role: MessageRole::User, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
    }

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages.iter().filter_map(|m| m.content.as_deref()).collect()
    }

    fn envelope(event: ActorEvent) -> Envelope {
        Envelope::new(SecurityContext::default(), EventContext::default(), event)
    }

    #[tokio::test]
    async fn all_it_hears_is_kept_and_what_others_said_with_text_is_given_once() {
        let alice = ActorId::User(UserId::new());
        let of_alice = |text: &str| Message { id: MessageId::new(), sender_id: alice, sender_name: "alice".to_string(), ..said(text) };
        let (first, second) = (of_alice("hello"), of_alice("how are you?"));
        let empty = Message { content: None, ..of_alice("") };
        let step = Message { internal: true, ..of_alice("thinking") };
        let ids = |messages: Vec<Message>| messages.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let body = AgentBody::detached(Vec::new());

        let mut state = AgentChannelState::default();
        body.perceive(&mut state, vec![first.clone()]).await;
        assert_eq!(ids(state.transcript.messages()), vec![first.id], "never sent anything: what others said");
        state.transcript.start_anew();
        body.perceive(&mut state, vec![empty.clone(), step.clone(), second.clone()]).await;
        assert_eq!(ids(messages_of(&state.history)), vec![first.id, empty.id, step.id, second.id], "all it heard is kept");
        assert_eq!(ids(state.transcript.messages()), vec![second.id], "after what was sent, with text");
        state.transcript.start_anew();
        assert!(state.transcript.messages().is_empty(), "nothing new");
    }

    #[tokio::test]
    async fn everything_queued_is_heard_in_order_and_only_the_last_request_is_answered() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let body = AgentBody::detached(Vec::new());

        let heard = |text: &str| envelope(ActorEvent::CompleteMessageGenerated(said(text)));
        let mut last = envelope(ActorEvent::ResponseRequested);
        last.security.caller_id = ActorId::System;
        for event in [envelope(ActorEvent::ResponseRequested), heard("two"), envelope(ActorEvent::ResponseStarted { turn_id: TurnId::new() }), last, heard("three")] {
            tx.try_send(event).expect("room");
        }

        let request = body.take_in(&mut state, heard("one"), &mut rx).await.request;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three"], "all of it, in order, also what came after the last request");
        assert_eq!(request.map(|r| r.security.caller_id), Some(ActorId::System), "the last request");
        assert!(rx.try_recv().is_none(), "nothing left behind");

        let request = body.take_in(&mut state, heard("four"), &mut rx).await.request;
        assert!(request.is_none(), "heard, but nobody asked");
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three", "four"]);
    }

    #[tokio::test]
    async fn a_command_ends_the_batch_and_what_follows_it_stays_queued() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let body = AgentBody::detached(Vec::new());
        let heard = |text: &str| envelope(ActorEvent::CompleteMessageGenerated(said(text)));
        let command = envelope(ActorEvent::CommandRequested { command: "mark".to_string(), args: String::new(), agent: ActorId::None });
        for event in [envelope(ActorEvent::ResponseRequested), heard("two"), command, heard("three"), envelope(ActorEvent::ResponseRequested)] {
            tx.try_send(event).expect("room");
        }

        let first = body.take_in(&mut state, heard("one"), &mut rx).await;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two"], "what was said before the command");
        assert!(matches!(first.command.as_ref().map(|c| &c.event), Some(ActorEvent::CommandRequested { .. })), "the command that ended the batch");
        assert!(first.request.is_some(), "the request before it is kept");

        // The actor carries it out here, then takes in the rest.
        let next = rx.try_recv().expect("what follows the command stays queued");
        let rest = body.take_in(&mut state, next, &mut rx).await;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three"], "in the order it was said");
        let all = first.then(rest);
        assert!(all.command.is_none() && all.request.is_some(), "no command left, and the later request");
    }

    #[tokio::test]
    async fn a_change_is_recorded_where_the_chains_hold_the_message_and_handed_over_where_the_transcript_does_not() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let (given, kept) = (Message { id: MessageId::new(), ..said("given") }, Message { id: MessageId::new(), ..said("kept") });
        state.perceive(vec![given.clone()], vec![given.clone()]);
        state.transcript.start_anew();
        state.perceive(vec![kept.clone()], vec![kept.clone()]);
        let modified = |message: &Message, text: &str| envelope(ActorEvent::MessageModified(Message { content: Some(text.to_string()), ..message.clone() }));
        let heard = |text: &str| envelope(ActorEvent::CompleteMessageGenerated(Message { id: MessageId::new(), ..said(text) }));
        // Writes down what it is handed, where it is handed it.
        fn noted<'a>(_: &'a ActorRuntime, state: &'a mut AgentChannelState, change: TranscriptEntry) -> BoxFuture<'a, ()> {
            let text = match change {
                TranscriptEntry::Modified { message } => format!("modified: {}", message.content.unwrap_or_default()),
                TranscriptEntry::Revoked { .. } => "taken back".to_string(),
                _ => "something else".to_string(),
            };
            state.transcript.push(Message { id: MessageId::new(), ..said(&text) });
            Box::pin(ready(()))
        }

        let noting = AgentBody::detached(Vec::new()).handing_over(noted);
        tx.try_send(modified(&given, "given, corrected")).expect("room");
        tx.try_send(heard("and then")).expect("room");
        tx.try_send(envelope(ActorEvent::MessageRevoked { message: given.id, at: Utc::now() })).expect("room");
        tx.try_send(envelope(ActorEvent::MessageRevoked { message: MessageId::new(), at: Utc::now() })).expect("room");
        noting.take_in(&mut state, modified(&kept, "kept, corrected"), &mut rx).await;

        assert_eq!(texts(&state.transcript.messages()), ["kept, corrected", "modified: given, corrected", "and then", "taken back", "taken back"],
            "on the active path the transcript takes it; off it, it is handed over where it came - also what the agent never held: whether that is anything to tell is the actor's to see");
        assert_eq!(state.history.message(kept.id).and_then(|m| m.content).as_deref(), Some("kept, corrected"), "the history holds both as they stand");
        assert!(state.history.message(given.id).is_some_and(|m| m.revoked.is_some()));

        // A modification of which nothing passes the processors takes the
        // message back in the transcript; the history keeps what was said.
        let stages: Vec<ComponentImpl<dyn PerceiveInProcessor>> = vec![ComponentImpl { id: "no-spam".to_string(), fail_policy: ComponentFailPolicy::FailClosed, facet: Arc::new(Without(Some("spam"))) }];
        AgentBody::detached(stages).handing_over(noted).take_in(&mut state, modified(&kept, "kept, spam"), &mut rx).await;
        assert_eq!(texts(&state.transcript.messages()), ["modified: given, corrected", "and then", "taken back", "taken back"], "no longer given, and nothing handed over");
        assert_eq!(state.history.message(kept.id).and_then(|m| m.content).as_deref(), Some("kept, spam"));

        // A reclassification is recorded where the chains hold the message,
        // and handed over nowhere: a remote agent knows no labels.
        let earlier = Message { id: MessageId::new(), ..said("earlier") };
        state.perceive(vec![earlier.clone()], vec![earlier.clone()]);
        state.transcript.start_anew();
        let (public, before) = (Classification::PUBLIC, state.transcript.all_entries().len());
        noting.take_in(&mut state, envelope(ActorEvent::MessageReclassified { message: earlier.id, classification: public.clone() }), &mut rx).await;
        assert_eq!(state.history.message(earlier.id).map(|m| m.classification), Some(public), "in the history");
        assert_eq!(state.transcript.all_entries().len(), before, "off the transcript's path: nothing recorded, nothing handed over");
    }
}
