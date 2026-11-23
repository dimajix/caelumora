//! Who takes part in a thread of a running channel (issue 602): whom a thread draws in - whoever
//! says something in it, and whom it mentions -, nobody where its circle is closed, and what
//! taking someone in and closing a thread make of someone drawn in meanwhile.

use super::*;
use crate::testing::shown;
use caelumora_domain::actor::Rejection;
use caelumora_domain::channel::{Delegation, ThreadOrigin};
use caelumora_domain::message::MessageId;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time;

/// A thread of `channel` at a message of its line that is not in its history, opened on behalf of
/// `context`: one nobody is drawn into as it is opened.
async fn aside(channel: &Arc<ChannelLoop>, context: &SecurityContext) -> ThreadId {
    let origin = ThreadOrigin { thread: ThreadId::line(channel.id()), message: MessageId::new() };
    channel.open_thread_by_procedure(context, origin, &[]).await.expect("opened")
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
                ActorEvent::ParticipantJoined { participant, .. } => {
                    let (there, by) = (envelope.context.thread_id, envelope.context.sender_id);
                    assert_eq!((there, by), (thread, hers), "there, in her name");
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
/// taken in after closing it was asked: `run` asks it again as it would close it.
#[tokio::test]
async fn a_thread_someone_came_into_since_closing_it_was_asked_stays_open() {
    let (channel, context, agent, _) = scripted_channel(Script::Answers("ok".to_string())).await;
    let helper_id = ActorId::Agent(agent);
    let thread = aside(&channel, &context).await;
    let joins = channel.take_into_thread(helper_id, thread, &context).await.expect("asked");
    let closing = tokio::spawn({
        let (channel, context) = (Arc::clone(&channel), context.clone());
        async move { channel.close_thread(&context, thread).await }
    });
    // Closing asked before `run` takes the join in, most likely; refused either way.
    time::sleep(Duration::from_millis(50)).await;
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });

    assert!(joins.carried_out().await, "taken in");
    let refused = closing.await.expect("ran").expect_err("someone takes part");
    let refused = refused.downcast_ref::<ChannelError>();
    assert!(matches!(refused, Some(ChannelError::ThreadNotEmpty { .. })), "{refused:?}");
    assert!(channel.is_open(thread), "still open");
    running.abort();
}
