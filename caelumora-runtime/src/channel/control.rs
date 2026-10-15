//! Controlling the actors of a channel: cancelling, waking, interrupting and stopping them.

use anyhow::Result;
use caelumora_model::actor::{ActorId, ActorState};
use caelumora_model::work::TurnId;
use caelumora_model::security::{ActionType, SecurityContext};
use tracing::warn;
use std::sync::atomic::Ordering;
use tokio_util::task::TaskTracker;
use super::{ChannelActor, ChannelError, ChannelLoop, stopped};

impl ChannelLoop {
    /// Ends the current turn of every actor in the channel, on behalf of
    /// `context`'s initiator, and keeps the channel quiet - nobody is asked
    /// to respond - until it is woken again, usually by the next message.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses
    /// `CancelChannel`; the channel is then untouched. Deliberately one
    /// decision for the whole channel: a per-actor one could leave half of it
    /// cancelled (issue 30).
    pub async fn cancel(&self, context: &SecurityContext) -> Result<()> {
        self.require(context, |attributes| ActionType::CancelChannel(attributes), "cancelling").await?;
        self.cancel_actors();
        Ok(())
    }

    /// Ends the current turn of every actor and keeps the channel quiet:
    /// what `cancel` does once permitted, and `reclassify` as a consequence
    /// of lowering the channel.
    pub(super) fn cancel_actors(&self) {
        self.quiet.store(true, Ordering::SeqCst);
        for ChannelActor { actor, .. } in self.actors.load().values() {
            actor.cancel();
        }
    }

    /// Wakes the channel's sleeping actors because someone asked for it -
    /// a `/wakeup` and nothing else. Returns the names of the actors that
    /// were asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses
    /// `WakeChannel`.
    pub async fn wakeup(&self, context: &SecurityContext) -> Result<Vec<String>> {
        self.require(context, |attributes| ActionType::WakeChannel(attributes), "waking the agents of").await?;
        let asleep = self.actors.load().values()
            .map(|a| &a.actor)
            .filter(|a| a.state() == ActorState::Sleeping)
            .map(|a| a.name().to_string())
            .collect();
        self.wake_for_delivery();
        Ok(asleep)
    }

    /// Ends the current turn of one actor deliberately (issue 235), like
    /// `cancel` and under the same permission: the policy that lets someone
    /// stop the channel lets them stop one of its actors. The channel is not
    /// kept quiet - that is the channel's state, not the actor's: the others
    /// are still asked to respond, and the answer the actor's turn is taken
    /// over with reaches everyone as any answer does.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `AccessPolicy` refuses
    /// `CancelChannel`, before anything of the actor is looked at;
    /// `NotAParticipant` if `actor` is not in this channel.
    pub async fn cancel_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<()> {
        self.require(context, |attributes| ActionType::CancelChannel(attributes), "cancelling").await?;
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
    pub async fn interrupt_tool_call(&self, context: &SecurityContext, actor: ActorId, turn: TurnId, call: &str) -> Result<()> {
        self.require(context, |attributes| ActionType::CancelChannel(attributes), "cancelling").await?;
        let actor = self.actor(actor)
            .ok_or(ChannelError::NotAParticipant { participant: actor, channel: self.id })?;
        // Named to the model as the channel knows them.
        let by = self.actor(context.initiator_id).map_or_else(|| "A user".to_string(), |initiator| initiator.name().to_string());
        if !actor.interrupt_tool_call(turn, call, by) {
            return Err(ChannelError::ToolCallNotRunning { call: call.to_string(), turn, channel: self.id }.into());
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
        self.require(context, |attributes| ActionType::WakeChannel(attributes), "waking the agents of").await?;
        let actor = self.actor(actor)
            .ok_or(ChannelError::NotAParticipant { participant: actor, channel: self.id })?;
        let was_asleep = actor.state() == ActorState::Sleeping;
        self.quiet.store(false, Ordering::SeqCst);
        actor.wakeup();
        Ok(was_asleep)
    }

    /// Tells every actor of the channel to stop for good (`Actor::stop`) -
    /// for a channel that is deleted: nothing of it is resumed, and whoever
    /// follows it through their actor is told it ended, whether or not the
    /// actors' tasks ever ran (issue 104). Returns once their tasks have
    /// ended, or `STOP_GRACE` has passed (issue 187): an external agent
    /// cancels its open remote tasks on its way out, and every actor stores
    /// its state consistent. Those still running then are the caller's to
    /// abort, with the channel's task.
    pub(crate) async fn stop_actors(&self) {
        let actors = self.actors.load();
        for ChannelActor { actor, .. } in actors.values() {
            actor.stop();
        }
        let tasks: Vec<TaskTracker> = actors.values().map(|a| a.task.clone()).collect();
        if !stopped(&tasks).await {
            warn!("Not every actor of channel {} stopped by itself: the rest is aborted", self.id);
        }
    }

    /// Wakes the channel's actors because something is on its way to them:
    /// a `WakeupRequested`, which a user's actor sends with every message. A
    /// mention wakes only whom it addresses (`route_event_impl`). That is the
    /// consequence of an action already permitted, so it carries no
    /// permission of its own - otherwise a user allowed to write but not to
    /// wake could not write.
    pub(super) fn wake_for_delivery(&self) {
        self.quiet.store(false, Ordering::SeqCst);
        for ChannelActor { actor, .. } in self.actors.load().values() {
            actor.wakeup();
        }
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::actor::ActorId;
    use caelumora_model::security::{ActionType, SecurityContext, AccessPolicy};
    use std::sync::Arc;
    use std::time::Duration;
    
    use caelumora_model::security::AllowAllPolicy;
    use crate::channel::tests::*;
    use caelumora_model::agent::AgentId;
    use crate::error::is_forbidden;

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
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let refused = channel_with(&[&alice], Arc::new(DenyAll)).await;
        // Refused before the actor is looked up: who may not learns nothing
        // of who is in the channel.
        let somebody = ActorId::Agent(AgentId::new());
        for error in [
            refused.cancel(&context).await.expect_err("cancelling is refused"),
            refused.cancel_actor(&context, somebody).await.expect_err("cancelling one actor is refused"),
            refused.wakeup(&context).await.expect_err("waking is refused"),
            refused.wakeup_actor(&context, somebody).await.expect_err("waking one actor is refused"),
        ] {
            assert!(is_forbidden(&error), "the caller can tell a refusal from a failure: {error}");
            assert!(!error.to_string().contains("DenyAll"), "a refusal does not disclose the rule: {error}");
        }
        // Waking because something is on its way is not a wakeup of its own:
        // it follows from an action that was permitted.
        refused.wake_for_delivery();

        let allowed = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        allowed.cancel(&context).await.expect("permitted");
        allowed.wakeup(&context).await.expect("permitted");
    }



    #[tokio::test]
    async fn a_stopped_channel_refuses_what_a_user_sends() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut said = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
        actor.send_message("hello".to_string(), None).await.expect("a running channel takes it");
        // Taken out of the inbox: `run` holds it now.
        tokio::time::timeout(Duration::from_secs(5), said.recv()).await.expect("the message went through").unwrap();

        // Aborted, as a stopped session aborts its channels: the inbox closes
        // with it, so a sender learns so instead of waiting for ever.
        running.abort();
        let _ = running.await;
        let refused = tokio::time::timeout(Duration::from_secs(5), actor.send_message("anyone?".to_string(), None)).await
            .expect("an answer, not a wait");
        assert!(refused.is_err(), "a stopped channel takes nothing");
    }
}
