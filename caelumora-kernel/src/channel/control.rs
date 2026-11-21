//! Controlling the actors of a channel: cancelling, waking, interrupting and stopping them.

use super::thread::Thread;
use super::{ActorChange, ChannelActor, ChannelError, ChannelLoop, Ending, LEAVE_GRACE};
use anyhow::Result;
use caelumora_domain::actor::{ActorId, ActorState};
use caelumora_domain::channel::ThreadId;
use caelumora_domain::work::TurnId;
use caelumora_security::{ActionType, SecurityContext};
use std::sync::Arc;
use tokio::time;
use tracing::warn;

impl ChannelLoop {
    /// Ends the current turn of every actor taking part in `thread` - in every thread of the
    /// channel if `None` -, on behalf of `context`'s initiator, and keeps the thread quiet - nobody
    /// is asked to respond in it - until it is woken again, usually by the next message said in it
    /// (issue 557).
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses `CancelChannel` - a thread is part
    /// of its channel -; the channel is then untouched. Deliberately one decision for the whole
    /// scope: a per-actor one could leave half of it cancelled (issue 30). `ThreadNotOpen` if
    /// `thread` is not open.
    pub async fn cancel(&self, context: &SecurityContext, thread: Option<ThreadId>) -> Result<()> {
        self.require(context, |attributes| ActionType::CancelChannel(attributes), "cancelling")
            .await?;
        self.require_open(thread)?;
        self.cancel_actors(thread);
        Ok(())
    }

    /// Ends the current turn of every actor taking part in `thread`, or in every thread if `None`,
    /// and keeps the thread quiet: what `cancel` does once permitted, and `reclassify` as a
    /// consequence of lowering the channel.
    pub(super) fn cancel_actors(&self, thread: Option<ThreadId>) {
        for held in self.threads_in(thread) {
            held.set_quiet(true);
        }
        let in_scope = |actor: &&ChannelActor| thread.is_none_or(|thread| actor.thread == thread);
        for ChannelActor { actor, .. } in self.actors.load().values().filter(in_scope) {
            actor.cancel();
        }
    }

    /// Wakes the sleeping actors of `thread` - of every thread if `None` - because someone asked
    /// for it - a `/wakeup` and nothing else -, and ends its being quiet. Returns the names of the
    /// actors that were asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses `WakeChannel`; `ThreadNotOpen` if
    /// `thread` is not open.
    pub async fn wakeup(
        &self,
        context: &SecurityContext,
        thread: Option<ThreadId>,
    ) -> Result<Vec<String>> {
        self.require(
            context,
            |attributes| ActionType::WakeChannel(attributes),
            "waking the agents of",
        )
        .await?;
        self.require_open(thread)?;
        let asleep = self
            .actors
            .load()
            .values()
            .filter(|a| thread.is_none_or(|thread| a.thread == thread))
            .map(|a| &a.actor)
            .filter(|a| a.state() == ActorState::Sleeping)
            .map(|a| a.name().to_string())
            .collect();
        self.wake(thread);
        Ok(asleep)
    }

    /// Nothing if `thread`, if one is named, is open in the channel.
    fn require_open(&self, thread: Option<ThreadId>) -> Result<()> {
        match thread {
            Some(thread) if !self.is_open(thread) => {
                Err(ChannelError::ThreadNotOpen { thread, channel: self.id }.into())
            }
            _ => Ok(()),
        }
    }

    /// The threads `thread` names: that one, if the channel holds it, or every one if `None`.
    fn threads_in(&self, thread: Option<ThreadId>) -> Vec<Arc<Thread>> {
        match thread {
            Some(thread) => self.thread(thread).into_iter().collect(),
            None => self.threads.load().values().map(Arc::clone).collect(),
        }
    }

    /// Ends the current turn of one actor deliberately (issue 235), like
    /// `cancel` and under the same permission: the policy that lets someone
    /// stop the channel lets them stop one of its actors. Its thread is not
    /// kept quiet - that is the thread's state, not the actor's: the others
    /// are still asked to respond, and the answer the actor's turn is taken
    /// over with reaches everyone as any answer does.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses
    /// `CancelChannel`, before anything of the actor is looked at;
    /// `NotAParticipant` if `actor` is not in this channel.
    pub async fn cancel_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<()> {
        self.require(context, |attributes| ActionType::CancelChannel(attributes), "cancelling")
            .await?;
        self.actor(actor)
            .ok_or(ChannelError::NotAParticipant { participant: actor, channel: self.id })?
            .cancel();
        Ok(())
    }

    /// Interrupts the tool call `call` of `actor`'s turn `turn` alone (issue
    /// 276), on behalf of `context`'s initiator: the call is answered as
    /// interrupted by them - or, not yet run, as not executed - and the turn
    /// goes on. Under the permission to cancel the channel, as
    /// `cancel_actor`: who may end a turn may end a part of it.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses
    /// `CancelChannel`, before anything of the actor is looked at;
    /// `NotAParticipant` if `actor` is not in this channel;
    /// `ToolCallNotRunning` if the call neither runs nor waits to run in
    /// that turn, or the turn is not under way.
    pub async fn interrupt_tool_call(
        &self,
        context: &SecurityContext,
        actor: ActorId,
        turn: TurnId,
        call: &str,
    ) -> Result<()> {
        self.require(context, |attributes| ActionType::CancelChannel(attributes), "cancelling")
            .await?;
        let actor = self
            .actor(actor)
            .ok_or(ChannelError::NotAParticipant { participant: actor, channel: self.id })?;
        // Named to the model as the channel knows them.
        let by = self
            .actor(context.initiator_id)
            .map_or_else(|| "A user".to_string(), |initiator| initiator.name().to_string());
        if !actor.interrupt_tool_call(turn, call, by) {
            return Err(ChannelError::ToolCallNotRunning {
                call: call.to_string(),
                turn,
                channel: self.id,
            }
            .into());
        }
        Ok(())
    }

    /// Wakes one actor deliberately, like `wakeup` and under the same
    /// permission. Returns whether it was asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses
    /// `WakeChannel`; `NotAParticipant` if `actor` is not in this channel.
    pub async fn wakeup_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<bool> {
        self.require(
            context,
            |attributes| ActionType::WakeChannel(attributes),
            "waking the agents of",
        )
        .await?;
        let ChannelActor { actor, thread, .. } = self
            .actors
            .load()
            .get(&actor)
            .cloned()
            .ok_or(ChannelError::NotAParticipant { participant: actor, channel: self.id })?;
        let was_asleep = actor.state() == ActorState::Sleeping;
        self.end_quiet(thread);
        actor.wakeup();
        Ok(was_asleep)
    }

    /// Ends every actor of the channel as `ending` says: told at once, past what is queued, to stop
    /// (`Actor::stop`), for a channel that stops with its session (issue 377) - what they are in
    /// the middle of is kept, to be resumed when the channel runs again -, or to leave for good
    /// (`Actor::leave`), for a channel deleted by itself or with its session - nothing of it is
    /// resumed, and whoever follows it through their actor is told it ended, whether or not its
    /// task ever ran (issue 104) -, as a participant let go is told (`drop_actor`). Then waits for
    /// `run` (`ActorChange::End`), which answers once their tasks have ended - what they sent until
    /// then is in the channel, an external agent's open remote tasks cancelled, every actor's state
    /// stored consistent - or were aborted after `STOP_GRACE` (issue 187); at once where `run`
    /// never ran. The channel's task is the caller's to abort.
    pub(crate) async fn end_actors(&self, ending: Ending) {
        for ChannelActor { actor, .. } in self.actors.load().values() {
            match ending {
                Ending::Stop => actor.stop(),
                Ending::Leave => actor.leave(),
            }
        }
        let ended = self.request(ActorChange::End);
        if !self.has_run() {
            return;
        }
        if time::timeout(LEAVE_GRACE, ended.carried_out()).await.is_err() {
            warn!("Not every actor of channel {} has ended", self.id);
        }
    }

    /// Wakes the actors of `thread` - of every thread if `None` - and ends its being quiet: what
    /// `wakeup` does once permitted, and what a `WakeupRequested` asks for, which a user's actor
    /// sends with every message, in the thread it is said in (issue 557). A mention wakes only
    /// whom it addresses (`route_event_impl`). That is the consequence of an action already
    /// permitted, so it carries no permission of its own - otherwise a user allowed to write but
    /// not to wake could not write.
    pub(super) fn wake(&self, thread: Option<ThreadId>) {
        for held in self.threads_in(thread) {
            held.set_quiet(false);
        }
        let in_scope = |actor: &&ChannelActor| thread.is_none_or(|thread| actor.thread == thread);
        for ChannelActor { actor, .. } in self.actors.load().values().filter(in_scope) {
            actor.wakeup();
        }
    }

    /// Ends `thread`'s being quiet, waking nobody: what a deliberate request of one ends - a nudge,
    /// a request to answer again, waking one actor -, whom it asks being woken alone.
    pub(super) fn end_quiet(&self, thread: ThreadId) {
        if let Some(held) = self.thread(thread) {
            held.set_quiet(false);
        }
    }

    /// Whether `thread` is quiet after a cancel: nobody is asked to respond in it.
    pub(super) fn is_quiet(&self, thread: ThreadId) -> bool {
        self.thread(thread).is_some_and(|held| held.is_quiet())
    }
}

#[cfg(test)]
mod tests {
    use crate::channel::tests::*;
    use crate::error::is_forbidden;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::user::testing::make_user;
    use caelumora_security::AllowAllPolicy;
    use caelumora_security::{AccessPolicy, ActionType, SecurityContext};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

    /// Refuses everything, to see that a refusal reaches the caller.
    #[derive(Debug)]
    struct DenyAll;

    #[async_trait::async_trait]
    impl AccessPolicy for DenyAll {
        async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn cancelling_and_waking_deliberately_ask_the_access_policy() {
        let alice = make_user("alice");
        let context =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let refused = channel_with(&[&alice], Arc::new(DenyAll)).await;
        // Refused before the actor is looked up: who may not learns nothing
        // of who is in the channel.
        let somebody = ActorId::Agent(AgentId::new());
        for error in [
            refused.cancel(&context, None).await.expect_err("cancelling is refused"),
            refused
                .cancel_actor(&context, somebody)
                .await
                .expect_err("cancelling one actor is refused"),
            refused.wakeup(&context, None).await.expect_err("waking is refused"),
            refused
                .wakeup_actor(&context, somebody)
                .await
                .expect_err("waking one actor is refused"),
        ] {
            assert!(is_forbidden(&error), "the caller can tell a refusal from a failure: {error}");
            assert!(
                !error.to_string().contains("DenyAll"),
                "a refusal does not disclose the rule: {error}"
            );
        }
        // Waking because something is on its way is not a wakeup of its own:
        // it follows from an action that was permitted.
        refused.wake(None);

        let allowed = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        allowed.cancel(&context, None).await.expect("permitted");
        allowed.wakeup(&context, None).await.expect("permitted");
    }

    #[tokio::test]
    async fn a_stopped_channel_refuses_what_a_user_sends() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut said = channel.observe();
        let running = tokio::spawn({
            let channel = Arc::clone(&channel);
            async move { channel.run().await }
        });
        actor
            .send_message(Some("hello".to_string()), None, Vec::new())
            .await
            .expect("a running channel takes it");
        // Taken out of the inbox: `run` holds it now.
        time::timeout(Duration::from_secs(5), said.recv())
            .await
            .expect("the message went through")
            .unwrap();

        // Aborted, as a stopped session aborts its channels: the inbox closes
        // with it, so a sender learns so instead of waiting for ever.
        running.abort();
        let _ = running.await;
        let refused = time::timeout(
            Duration::from_secs(5),
            actor.send_message(Some("anyone?".to_string()), None, Vec::new()),
        )
        .await
        .expect("an answer, not a wait");
        assert!(refused.is_err(), "a stopped channel takes nothing");
    }
}
