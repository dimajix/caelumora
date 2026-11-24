//! Who takes part in a thread of a running channel (issue 602): whom a thread draws in - whoever
//! says something in it, and whom it mentions by name -, nobody where its circle is closed, and
//! what taking someone in and closing a thread make of someone drawn in meanwhile.

use super::*;
use crate::testing::shown;
use caelumora_domain::actor::Rejection;
use caelumora_domain::channel::{Delegation, ThreadOrigin};
use caelumora_domain::history::HistoryEntry;
use caelumora_domain::message::{Message, MessageId};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time;

/// A thread of `channel` at a message of its line that is not in its history, opened on behalf of
/// `context`: one nobody is drawn into as it is opened.
async fn aside(channel: &Arc<ChannelLoop>, context: &SecurityContext) -> ThreadId {
    let origin = ThreadOrigin { thread: ThreadId::line(channel.id()), message: MessageId::new() };
    channel.open_thread_by_procedure(context, ThreadSpec::new(origin)).await.expect("opened")
}

/// Issue 602: whoever says something in a thread and whom it mentions take part in it from then
/// on, their joins announced there in the name of who said it, before what they hear first; the
/// agent mentioned is asked there and answers there.
#[tokio::test]
async fn whoever_says_something_in_a_thread_and_whom_it_mentions_take_part_in_it() {
    let Stuck { channel, context, agent, helper, mut events, running } =
        start(scripted_channel(Script::Answers("Here.".to_string())).await).await;
    let (hers, helper_id) = (ActorId::User(alice_of(&context)), ActorId::Agent(agent));
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    let thread = aside(&channel, &context).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();

    let word = "@helper, a word";
    actor.send_message(Some(thread), Some(word.to_string()), None, Vec::new()).await.unwrap();
    let mut joined = Vec::new();
    let answer = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("open");
            match &envelope.event {
                ActorEvent::ParticipantJoined { participant, into_channel, .. } => {
                    let (there, by) = (envelope.context.thread_id, envelope.context.sender_id);
                    assert_eq!((there, by), (thread, hers), "there, in her name");
                    assert!(!into_channel, "into the thread, not the channel (issue 603)");
                    joined.push(*participant);
                }
                ActorEvent::CompleteMessageGenerated(said)
                    if said.content.as_deref() == Some(word) =>
                {
                    assert_eq!(
                        joined,
                        [hers, helper_id],
                        "who said it and whom it mentions, first"
                    );
                }
                ActorEvent::TurnFinished { .. } if envelope.context.sender_id == helper_id => {
                    break envelope;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the helper answers");
    assert_eq!(answer.context.thread_id, thread, "there");
    assert!(channel.runs_in(hers, thread) && channel.runs_in(helper_id, thread), "both take part");
    running.abort();
}

/// Issue 602: a thread whose circle is closed draws nobody in, and what one outside it says there
/// is not let in.
#[tokio::test]
async fn a_closed_circle_draws_nobody_in_and_refuses_whoever_is_outside() {
    let Stuck { channel, context, agent, helper, mut events, running } =
        start(scripted_channel(Script::Answers("Here.".to_string())).await).await;
    let (hers, helper_id) = (ActorId::User(alice_of(&context)), ActorId::Agent(agent));
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    let briefs = VecDeque::new();
    let delegation = Delegation { giver: hers, recipient: helper_id, initiator: hers, briefs };
    let closed = channel.forge_delegate(delegation, |_| Vec::new()).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let mut shown_to_her = actor.watch();

    let word = Some("@helper, in here".to_string());
    actor.send_message(Some(closed), word, None, Vec::new()).await.unwrap();
    let ActorEvent::EventRejected { rejected, reason, .. } =
        shown(&mut shown_to_her, "EventRejected").await
    else {
        unreachable!()
    };
    let refused = (rejected.as_str(), reason);
    assert_eq!(refused, ("CompleteMessageGenerated", Rejection::ClosedCircle), "not let in");
    assert!(!channel.runs_in(hers, closed), "she is not drawn in");
    assert!(!channel.runs_in(helper_id, closed), "nor whom she mentions");
    running.abort();
}

/// Issue 602: taking someone into a thread who was drawn in since it was asked - here, asked twice
/// before `run` took either in - is carried out, and their join announced once.
#[tokio::test]
async fn a_join_of_someone_in_the_thread_meanwhile_is_carried_out_and_announced_once() {
    let (channel, context, agent, _) = scripted_channel(Script::Answers("ok".to_string())).await;
    let helper_id = ActorId::Agent(agent);
    let thread = aside(&channel, &context).await;
    let first = channel.take_into_thread(helper_id, thread, &context).await.expect("asked");
    let again = channel.take_into_thread(helper_id, thread, &context).await.expect("not in yet");
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });

    assert!(first.carried_out().await && again.carried_out().await, "both carried out");
    assert!(channel.runs_in(helper_id, thread), "in the thread");
    let mut joins = 0;
    while let Ok(envelope) = events.try_recv() {
        let joined = matches!(
            envelope.event,
            ActorEvent::ParticipantJoined { participant, .. } if participant == helper_id
        );
        joins += usize::from(joined && envelope.context.thread_id == thread);
    }
    assert_eq!(joins, 1, "announced once");
    running.abort();
}

/// Issue 602: a thread is not closed while someone takes part in it whom it drew in or who was
/// taken in after closing it was asked - here, a closing asked while a join waits for `run`, which
/// takes the join in first: `run` asks again as it would close it and leaves it open, and whoever
/// asked finds it open. Whether the closing was handed to `run` is read off what waits for it, as
/// nothing else tells.
#[tokio::test]
async fn a_thread_someone_came_into_since_closing_it_was_asked_stays_open() {
    let (channel, context, agent, _) = scripted_channel(Script::Answers("ok".to_string())).await;
    let helper_id = ActorId::Agent(agent);
    let thread = aside(&channel, &context).await;
    let joins = channel.take_into_thread(helper_id, thread, &context).await.expect("asked");
    let waiting = || channel.requests_rx.lock().unwrap().as_ref().map_or(0, |rx| rx.len());
    let before = waiting();
    let closing = tokio::spawn({
        let (channel, context) = (Arc::clone(&channel), context.clone());
        async move { channel.close_thread(&context, thread).await }
    });
    time::timeout(Duration::from_secs(5), async {
        while waiting() == before {
            time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the closing handed to `run`, behind the join");
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });

    assert!(joins.carried_out().await, "taken in");
    let refused = closing.await.expect("ran").expect_err("someone takes part");
    let refused = refused.downcast_ref::<ChannelError>();
    assert!(matches!(refused, Some(ChannelError::ThreadNotEmpty { .. })), "{refused:?}");
    assert!(channel.is_open(thread), "left open");
    while let Ok(envelope) = events.try_recv() {
        let closed = matches!(envelope.event, ActorEvent::ThreadClosed { .. });
        assert!(!closed, "and not announced closed");
    }
    running.abort();
}

/// Issue 602: a group handle mentions nobody by name: said in a thread, it draws in whoever said it
/// alone, and nobody is told of anyone not brought in.
#[tokio::test]
async fn a_group_handle_draws_nobody_into_a_thread() {
    let Stuck { channel, context, agent, helper, mut events, running } =
        start(scripted_channel(Script::Answers("Here.".to_string())).await).await;
    let (hers, helper_id) = (ActorId::User(alice_of(&context)), ActorId::Agent(agent));
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    let thread = aside(&channel, &context).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();

    let word = "@all, a word";
    actor.send_message(Some(thread), Some(word.to_string()), None, Vec::new()).await.unwrap();
    let said = shown_saying(&mut events, word).await;
    // What the moderator tells of it is recorded before the channel lets others see it.
    let told = channel.history_entries().await;
    assert!(
        !told.iter().any(|entry| matches!(entry, HistoryEntry::NotDrawnIn { .. })),
        "nobody named, nobody told of"
    );
    assert!(said.mentions.is_empty(), "nobody by name");
    // The helper takes part in the line alone, and a group handle asks nothing of who says it.
    assert!(said.addressees.is_empty(), "nobody addressed (issue 594): {:?}", said.addressees);
    assert!(channel.runs_in(hers, thread), "who said it takes part");
    assert!(!channel.runs_in(helper_id, thread), "the helper is not drawn in");
    running.abort();
}

/// The message saying `text` that `events` show next - at most five seconds, after which the test
/// fails - as it was said, or as it was modified to say it.
async fn shown_saying(events: &mut broadcast::Receiver<Envelope>, text: &str) -> Message {
    time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await.expect("open").event {
                ActorEvent::CompleteMessageGenerated(said) | ActorEvent::MessageModified(said)
                    if said.content.as_deref() == Some(text) =>
                {
                    break said;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("said")
}

/// Issue 594: a group handle said in a thread addresses who takes part there alone, but whoever
/// says it - not the helper, which takes part in the line -, also as the message is modified, which
/// happens in the whole channel; said in the line, it addresses the helper there.
#[tokio::test]
async fn a_group_handle_addresses_who_takes_part_in_the_thread_it_is_said_in() {
    let Stuck { channel, context, agent, helper, mut events, running } =
        start(scripted_channel(Script::Answers("Here.".to_string())).await).await;
    let helper_id = ActorId::Agent(agent);
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    let thread = aside(&channel, &context).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let say = |thread, text: &str| actor.send_message(thread, Some(text.into()), None, Vec::new());

    say(Some(thread), "@all, a word").await.unwrap();
    let said = shown_saying(&mut events, "@all, a word").await;
    assert!(said.addressees.is_empty(), "nobody else takes part: {:?}", said.addressees);

    actor
        .modify_message(said.id, Some("@all, a word more".into()), None, Vec::new())
        .await
        .unwrap();
    let modified = shown_saying(&mut events, "@all, a word more").await;
    assert!(modified.addressees.is_empty(), "where it was said: {:?}", modified.addressees);

    say(None, "@all, in the line").await.unwrap();
    let in_line = shown_saying(&mut events, "@all, in the line").await;
    assert_eq!(in_line.addressees, [helper_id], "who takes part in the line, but alice");
    running.abort();
}
