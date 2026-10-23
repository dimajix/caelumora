//! How an actor of an agent takes in what it is told (issues 196, 206, 222,
//! 234), the same for both kinds of agent - the `AgentActor` and the
//! `ExternalAgentActor`: what others said, as it passes the agent's perceive
//! processors, a message modified or taken back since - into its
//! transcript, and where it stands in its channel's history into its state
//! (issue 223); what it was
//! offered is its channel's history (issue 417). Its body's (`AgentBody`).

use super::body::AgentBody;
use crate::message::apply_stage_processors;
use caelumora_runtime::logging::envelope_span;
use caelumora_runtime::transport::EventReceiver;
use caelumora_model::actor::{ActorEvent, Envelope};
use caelumora_model::agent::{AgentChannelState, HearsFrom};
use caelumora_model::message::Message;
use caelumora_model::transcript::TranscriptEntry;
use tracing::{debug, trace, warn, Instrument};

impl AgentBody {
    /// Takes what others said, `heard`, into the agent's `state` (issues
    /// 196, 206, 224), the same for both kinds of agent and every way in:
    /// what it hears of it - `Message::as_heard` - as it passes its perceive
    /// processors, into its transcript. A failing stage is logged and leaves
    /// the transcript as it was: what was heard stays in the channel's
    /// history, and is not offered again.
    ///
    /// What is heard is never the agent's own: the channel delivers nothing
    /// back to its sender (`ChannelLoop::route_event_impl`), live or from its
    /// history, and every message an envelope carries is its sender's
    /// (`OriginEventSender`).
    async fn perceive(&self, state: &mut AgentChannelState, heard: Vec<Message>) {
        let Some(last) = heard.last() else {
            return;
        };
        state.last_message = Some(last.id);
        let offered = heard.iter().filter_map(Message::as_heard).collect();
        let perceived = match apply_stage_processors(&self.perceive_in_processors, offered, &self.security_context()).await {
            Ok(perceived) => perceived,
            Err(e) => {
                warn!("The perceive stage failed: {:#}", e);
                Vec::new()
            }
        };
        state.perceive(perceived);
    }

    /// Takes in that a message of another reads as `modified` from now on
    /// (issue 234), the same for both kinds of agent - never its own, as
    /// nothing heard is (`perceive`): as the agent hears it -
    /// `Message::as_heard`, through its perceive processors, as everything
    /// heard - into its transcript, if the message is on its active path,
    /// as what its processors made of it under the message's id. Where nothing is left
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
    /// already behind it, in order. What was said, changed or noted is taken
    /// in as it comes (`take_in_said`). Of the requests to respond the last is
    /// returned - one answer to the conversation as it stands, in the
    /// authority of the last who asked - and the others are dropped, as is
    /// every other event but the requests to answer again (issue 234), which
    /// are returned in order, each to be answered. A command it is asked to
    /// carry out (issue 269) ends the batch: it is returned, and what follows
    /// it stays in `rx`, so that the actor carries it out where it came -
    /// after what was said before it, before what was said after - and then
    /// takes in the rest. Where the agent stands in its channel's history
    /// is written into `state` with what it took in (issue 223), so that
    /// every store that follows stores both; when that happens is the
    /// actor's.
    pub(super) async fn take_in(&self, state: &mut AgentChannelState, received: Envelope, rx: &mut Box<dyn EventReceiver>) -> TakenIn {
        let mut request = None;
        let mut retries = Vec::new();
        let mut next = Some(received);
        while let Some(event) = next {
            if matches!(event.event, ActorEvent::CommandRequested { .. }) {
                state.heard = rx.heard();
                return TakenIn { request, retries, command: Some(event) };
            }
            let span = envelope_span(&event.context, event.event.kind());
            let asked = async {
                debug!("Received {}", event.describe());
                let event = self.take_in_said(state, event).await?;
                match event.event {
                    ActorEvent::ResponseRequested { .. } => Some(event),
                    ActorEvent::RetryRequested { .. } => {
                        retries.push(event);
                        None
                    }
                    _ => None,
                }
            }.instrument(span).await;
            request = answered(request, asked);
            next = rx.try_recv().await;
        }
        state.heard = rx.heard();
        TakenIn { request, retries, command: None }
    }

    /// Takes in what `rx` holds already while a turn is under way in `state`
    /// (issue 431), between two of its steps - before its model is called
    /// again, so that the call is given it: what others said, as `take_in`
    /// perceives it, and who joined or left. What its personality hears
    /// then, and what the turn may hear at all, is the model's to say
    /// (`HearsDuringTurn::pick`). A request to respond it takes is dropped:
    /// the turn answers it, its message heard. It takes up to the first
    /// event it may not take and leaves that event and everything behind it
    /// where they are, in order, for `take_in` to take after the turn -
    /// unless a request in the turn's own authority lies further on, which
    /// lets it hear what waited only for the authority it was said in
    /// (`HearsDuringTurn::pick_request`, issue 435): it takes that request
    /// ahead and then what came before it, passing over the requests of
    /// others, which wait for a turn in theirs. Returns whether it took in
    /// anything. Writes where the agent stands into `state`, as `take_in`
    /// does, and does not store.
    pub(super) async fn take_in_during_turn(&self, state: &mut AgentChannelState, rx: &mut Box<dyn EventReceiver>) -> bool {
        let hears = self.hears_during_turn();
        let Some(under_way) = state.turn.as_ref().filter(|_| hears.from != HearsFrom::None) else {
            return false;
        };
        let work = under_way.work.clone();
        let mut taken = false;
        // Where the last request in the turn's authority taken ahead was.
        let mut asked = None;
        loop {
            while let Some(event) = rx.try_recv_if(&|envelope: &Envelope| hears.pick(&work, envelope, asked)).await {
                taken = true;
                let span = envelope_span(&event.context, event.event.kind());
                async {
                    debug!("Received {} during a turn", event.describe());
                    // A request left over is answered by the turn under way.
                    self.take_in_said(state, event).await;
                }.instrument(span).await;
            }
            let Some(request) = rx.try_recv_if(&|envelope: &Envelope| hears.pick_request(&work, envelope)).await else {
                break;
            };
            debug!("Received {} during a turn, ahead of what waited before it", request.describe());
            taken = true;
            asked = request.context.position;
        }
        state.heard = rx.heard();
        taken
    }

    /// Takes in `event` if it is something said, changed or noted - the
    /// same between turns (`take_in`) and during one
    /// (`take_in_during_turn`): what others said is perceived as it comes
    /// (`perceive`), and so is a message modified or taken back since
    /// (issue 234); a reclassification is recorded as it is (issue 239),
    /// in the transcript where it holds the message on its active path, and
    /// so is who joined or left (issue 408). A modification or revocation
    /// the transcript did not take is handed over (`AgentBody::hand_over`,
    /// issue 236), as the agent hears it. Returns any other event, for the
    /// caller to handle.
    async fn take_in_said(&self, state: &mut AgentChannelState, event: Envelope) -> Option<Envelope> {
        match event.event {
            ActorEvent::CompleteMessageGenerated(message) => {
                trace!("Heard {:?}", message.content);
                self.perceive(state, vec![message]).await;
            }
            ActorEvent::MessageModified(modified) => {
                if let Some(change) = self.take_in_modified(state, modified).await {
                    (self.hand_over)(&self.runtime, state, change).await;
                }
            }
            ActorEvent::MessageRevoked { message, at } => {
                if !state.transcript.revoke(message, at) {
                    (self.hand_over)(&self.runtime, state, TranscriptEntry::Revoked { message, at }).await;
                }
            }
            ActorEvent::MessageReclassified { message, classification } => {
                state.transcript.reclassify(message, classification);
            }
            // Who joined or left (issue 408): noted where it came, asking
            // nothing of the agent.
            ActorEvent::ParticipantJoined { .. } | ActorEvent::ParticipantLeft { .. } => {
                if let Some(entry) = event.announced() {
                    state.note_announced(entry);
                }
            }
            _ => return Some(event),
        }
        None
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
        TakenIn { request: answered(self.request, next.request), retries, command: next.command }
    }
}

/// Which of two requests to respond, `earlier` and `later`, both taken in
/// for one turn, the turn answers: the later, in whose contexts it acts -
/// for the stronger reason of the two (issue 439), since a mention among
/// them is a reason to answer though the last request was only a choice.
fn answered(earlier: Option<Envelope>, later: Option<Envelope>) -> Option<Envelope> {
    let Some(mut later) = later else {
        return earlier;
    };
    let before = earlier.and_then(|earlier| earlier.event.request_reason());
    if let (Some(before), ActorEvent::ResponseRequested { reason }) = (before, &mut later.event) {
        *reason = (*reason).max(before);
    }
    Some(later)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{HearsDuringTurn, OthersWords};
    use caelumora_model::chain::Position;
    use caelumora_runtime::component::ComponentImpl;
    use crate::message::testing::Without;
    use crate::message::PerceiveInProcessor;
    use caelumora_model::actor::{ActorId, EventContext, RequestReason};
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
    use caelumora_model::work::Work;
    use tokio::sync::mpsc;

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
    async fn what_others_said_with_text_is_given_once() {
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
        assert_eq!(ids(state.transcript.messages()), vec![second.id], "after what was sent, with text");
        state.transcript.start_anew();
        assert!(state.transcript.messages().is_empty(), "nothing new");
    }

    #[tokio::test]
    async fn everything_queued_is_heard_in_order_and_only_the_last_request_is_answered() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let body = AgentBody::detached(Vec::new());

        let heard = |text: &str| envelope(ActorEvent::CompleteMessageGenerated(said(text)));
        let mut last = envelope(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned });
        last.security.caller_id = ActorId::System;
        for event in [envelope(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }), heard("two"), envelope(ActorEvent::ResponseStarted { turn_id: TurnId::new() }), last, heard("three")] {
            tx.try_send(event).expect("room");
        }

        let request = body.take_in(&mut state, heard("one"), &mut rx).await.request;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three"], "all of it, in order, also what came after the last request");
        assert_eq!(request.map(|r| r.security.caller_id), Some(ActorId::System), "the last request");
        assert!(rx.try_recv().await.is_none(), "nothing left behind");

        let request = body.take_in(&mut state, heard("four"), &mut rx).await.request;
        assert!(request.is_none(), "heard, but nobody asked");
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three", "four"]);
    }

    /// Issue 439: of the requests a turn takes in, it answers the last - in
    /// whose contexts it acts - for the strongest reason among them, also
    /// across a command that ended a batch.
    #[tokio::test]
    async fn the_request_answered_keeps_the_strongest_reason() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let body = AgentBody::detached(Vec::new());
        let asked = |reason| envelope(ActorEvent::ResponseRequested { reason });
        let reason = |taken: &TakenIn| match taken.request.as_ref().map(|r| &r.event) {
            Some(ActorEvent::ResponseRequested { reason }) => Some(*reason),
            _ => None,
        };

        let mut last = asked(RequestReason::Chosen);
        last.security.caller_id = ActorId::System;
        tx.try_send(last).expect("room");
        let taken = body.take_in(&mut state, asked(RequestReason::Mentioned), &mut rx).await;
        assert_eq!(taken.request.as_ref().map(|r| r.security.caller_id), Some(ActorId::System), "the last request");
        assert_eq!(reason(&taken), Some(RequestReason::Mentioned), "for the mention before it");

        tx.try_send(asked(RequestReason::Mentioned)).expect("room");
        let taken = body.take_in(&mut state, asked(RequestReason::Chosen), &mut rx).await;
        assert_eq!(reason(&taken), Some(RequestReason::Mentioned), "a later mention is the stronger reason as well");

        let only_chosen = body.take_in(&mut state, asked(RequestReason::Chosen), &mut rx).await;
        assert_eq!(reason(&only_chosen), Some(RequestReason::Chosen), "chosen alone, chosen");
        let then = TakenIn { request: Some(asked(RequestReason::Nudged)), retries: Vec::new(), command: None }.then(only_chosen);
        assert_eq!(reason(&then), Some(RequestReason::Nudged), "across a command, too");
    }

    /// A turn begun for `initiator` in a channel classified as the default,
    /// in `state`.
    fn under_way_for(state: &mut AgentChannelState, initiator: ActorId) {
        let context = EventContext { initiator_id: initiator, ..EventContext::default() };
        state.begin_turn(Work::new(context, None, SecurityContext::default(), Classification::CHANNEL_DEFAULT, Default::default()));
    }

    #[tokio::test]
    async fn during_a_turn_what_may_be_heard_is_taken_up_to_the_first_event_that_has_to_wait() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let alice = ActorId::User(UserId::new());
        let mut state = AgentChannelState::default();
        under_way_for(&mut state, alice);
        let body = AgentBody::detached(Vec::new()).hearing(HearsDuringTurn { from: HearsFrom::Users, ..Default::default() });
        let in_her_authority = |event: ActorEvent| Envelope::new(SecurityContext::default(), EventContext { initiator_id: alice, ..EventContext::default() }, event);
        let of_alice = |text: &str| in_her_authority(ActorEvent::CompleteMessageGenerated(Message { sender_id: alice, ..said(text) }));
        let command = in_her_authority(ActorEvent::CommandRequested { command: "mark".to_string(), args: String::new(), agent: ActorId::None });
        for event in [of_alice("one"), in_her_authority(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }), of_alice("two"), command, of_alice("three")] {
            tx.try_send(event).expect("room");
        }

        assert!(body.take_in_during_turn(&mut state, &mut rx).await, "it took something in");
        assert_eq!(texts(&state.transcript.messages()), ["one", "two"], "what was said before the command");
        let turn = state.turn.as_ref().expect("still under way");
        assert_eq!(turn.work.context.transcript, state.transcript.digest(), "the turn's next call is given it");
        let mut left = Vec::new();
        while let Some(event) = rx.try_recv().await {
            left.push(event.event.kind());
        }
        assert_eq!(left, ["CommandRequested", "CompleteMessageGenerated"], "the command and what follows it wait, in order; the request was the turn's");
    }

    /// What bob and alice say during alice's turn, each at its position:
    /// bob's words and request, alice's words and request, bob's words
    /// and request again.
    fn bob_then_alice_then_bob(alice: ActorId, bob: ActorId) -> Vec<Envelope> {
        let at = |seq: u64, initiator: ActorId, event: ActorEvent| {
            Envelope::new(SecurityContext::default(), EventContext { initiator_id: initiator, position: Some(Position::new(seq)), ..EventContext::default() }, event)
        };
        let of = |seq, sender: ActorId, text: &str| at(seq, sender, ActorEvent::CompleteMessageGenerated(Message { sender_id: sender, ..said(text) }));
        vec![
            of(0, bob, "his first"), at(1, bob, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
            of(2, alice, "hers"), at(3, alice, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
            of(4, bob, "his second"), at(5, bob, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
        ]
    }

    /// What `rx` holds still, as the kinds of its events and their
    /// positions.
    async fn left_in(rx: &mut Box<dyn EventReceiver>) -> Vec<(&'static str, u64)> {
        let mut left = Vec::new();
        while let Some(event) = rx.try_recv().await {
            left.push((event.event.kind(), event.context.position.map_or(u64::MAX, |seq| seq.get())));
        }
        left
    }

    /// Issue 435: with `others: when_asked`, alice asking after bob spoke
    /// lets her turn hear him up to her request; his requests wait, and
    /// what he says after it waits too.
    #[tokio::test]
    async fn during_a_turn_its_initiator_asking_again_lets_it_hear_what_another_said_before() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let mut state = AgentChannelState::default();
        under_way_for(&mut state, alice);
        for event in bob_then_alice_then_bob(alice, bob) {
            tx.try_send(event).expect("room");
        }
        let body = AgentBody::detached(Vec::new()).hearing(HearsDuringTurn { from: HearsFrom::Users, others: OthersWords::WhenAsked });

        assert!(body.take_in_during_turn(&mut state, &mut rx).await, "it took something in");
        assert_eq!(texts(&state.transcript.messages()), ["his first", "hers"], "what was said before her request, in order");
        assert!(state.heard.has(Position::new(3)) && !state.heard.has(Position::new(1)), "where it stands: his request not taken");
        assert_eq!(left_in(&mut rx).await, [("ResponseRequested", 1), ("CompleteMessageGenerated", 4), ("ResponseRequested", 5)], "his requests and what he said after hers wait, in order");
    }

    /// Issue 435: with `others: wait`, or when only bob asks, nothing he
    /// said is heard.
    #[tokio::test]
    async fn during_a_turn_anothers_request_or_waiting_lets_it_hear_nothing_of_him() {
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        for (others, said) in [(OthersWords::Wait, bob_then_alice_then_bob(alice, bob)), (OthersWords::WhenAsked, bob_then_alice_then_bob(alice, bob).into_iter().filter(|e| e.context.initiator_id == bob).collect())] {
            let (tx, rx) = mpsc::channel(8);
            let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
            let mut state = AgentChannelState::default();
            under_way_for(&mut state, alice);
            let count = said.len();
            for event in said {
                tx.try_send(event).expect("room");
            }
            let body = AgentBody::detached(Vec::new()).hearing(HearsDuringTurn { from: HearsFrom::Users, others });
            assert!(!body.take_in_during_turn(&mut state, &mut rx).await, "{others:?}: nothing taken in");
            assert_eq!(left_in(&mut rx).await.len(), count, "{others:?}: all of it waits");
        }
    }

    /// Issue 435: a command between bob's words and alice's request stays
    /// a barrier: her request lifts nothing past it.
    #[tokio::test]
    async fn during_a_turn_a_command_before_its_initiators_request_lifts_nothing() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let mut state = AgentChannelState::default();
        under_way_for(&mut state, alice);
        let mut said = bob_then_alice_then_bob(alice, bob);
        let command = ActorEvent::CommandRequested { command: "mark".to_string(), args: String::new(), agent: ActorId::None };
        said.insert(2, Envelope::new(SecurityContext::default(), EventContext { initiator_id: alice, ..EventContext::default() }, command));
        for event in said {
            tx.try_send(event).expect("room");
        }
        let body = AgentBody::detached(Vec::new()).hearing(HearsDuringTurn { from: HearsFrom::Users, others: OthersWords::WhenAsked });
        assert!(!body.take_in_during_turn(&mut state, &mut rx).await, "nothing taken in");
        assert_eq!(left_in(&mut rx).await.len(), 7, "all of it waits");
    }

    #[tokio::test]
    async fn between_turns_nothing_is_taken_in_as_during_one() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        tx.try_send(envelope(ActorEvent::CompleteMessageGenerated(said("one")))).expect("room");
        let body = AgentBody::detached(Vec::new()).hearing(HearsDuringTurn { from: HearsFrom::All, ..Default::default() });
        assert!(!body.take_in_during_turn(&mut state, &mut rx).await, "no turn, nothing taken");
        assert!(rx.try_recv().await.is_some(), "it waits for take_in");
    }

    #[tokio::test]
    async fn a_command_ends_the_batch_and_what_follows_it_stays_queued() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let body = AgentBody::detached(Vec::new());
        let heard = |text: &str| envelope(ActorEvent::CompleteMessageGenerated(said(text)));
        let command = envelope(ActorEvent::CommandRequested { command: "mark".to_string(), args: String::new(), agent: ActorId::None });
        for event in [envelope(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }), heard("two"), command, heard("three"), envelope(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned })] {
            tx.try_send(event).expect("room");
        }

        let first = body.take_in(&mut state, heard("one"), &mut rx).await;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two"], "what was said before the command");
        assert!(matches!(first.command.as_ref().map(|c| &c.event), Some(ActorEvent::CommandRequested { .. })), "the command that ended the batch");
        assert!(first.request.is_some(), "the request before it is kept");

        // The actor carries it out here, then takes in the rest.
        let next = rx.try_recv().await.expect("what follows the command stays queued");
        let rest = body.take_in(&mut state, next, &mut rx).await;
        assert_eq!(texts(&state.transcript.messages()), ["one", "two", "three"], "in the order it was said");
        let all = first.then(rest);
        assert!(all.command.is_none() && all.request.is_some(), "no command left, and the later request");
    }

    #[tokio::test]
    async fn a_change_is_recorded_where_the_transcript_holds_the_message_and_handed_over_where_it_does_not() {
        let (tx, rx) = mpsc::channel(8);
        let mut rx: Box<dyn EventReceiver> = Box::new(InMemoryEventReceiver::new(rx));
        let mut state = AgentChannelState::default();
        let (given, kept) = (Message { id: MessageId::new(), ..said("given") }, Message { id: MessageId::new(), ..said("kept") });
        state.perceive(vec![given.clone()]);
        state.transcript.start_anew();
        state.perceive(vec![kept.clone()]);
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

        // A modification of which nothing passes the processors takes the
        // message back in the transcript; the channel's history keeps what
        // was said (issue 417).
        let stages: Vec<ComponentImpl<dyn PerceiveInProcessor>> = vec![ComponentImpl { id: "no-spam".to_string(), fail_policy: ComponentFailPolicy::FailClosed, facet: Arc::new(Without(Some("spam"))) }];
        AgentBody::detached(stages).handing_over(noted).take_in(&mut state, modified(&kept, "kept, spam"), &mut rx).await;
        assert_eq!(texts(&state.transcript.messages()), ["modified: given, corrected", "and then", "taken back", "taken back"], "no longer given, and nothing handed over");

        // A reclassification is recorded where the transcript holds the
        // message, and handed over nowhere: a remote agent knows no labels.
        let earlier = Message { id: MessageId::new(), ..said("earlier") };
        state.perceive(vec![earlier.clone()]);
        state.transcript.start_anew();
        let (public, before) = (Classification::PUBLIC, state.transcript.all_entries().len());
        noting.take_in(&mut state, envelope(ActorEvent::MessageReclassified { message: earlier.id, classification: public.clone() }), &mut rx).await;
        assert_eq!(state.transcript.all_entries().len(), before, "off the transcript's path: nothing recorded, nothing handed over");
        let mut on_path = AgentChannelState::default();
        on_path.perceive(vec![earlier.clone()]);
        noting.take_in(&mut on_path, envelope(ActorEvent::MessageReclassified { message: earlier.id, classification: public.clone() }), &mut rx).await;
        assert_eq!(on_path.transcript.messages().first().map(|m| m.classification.clone()), Some(public), "on it, recorded");
    }
}
