//! Delegating into the threads of a running channel (issue 557): a brief given by a thread's
//! delegate to its recipient, and what came of it brought back to its giver where the thread was
//! opened - each thread's delegation kept in the channel's state, which the channel alone writes.

use super::participation::ResolvedParticipant;
use super::run::RunningActor;
use super::thread::Phase;
use super::{ActorChange, ChannelError, ChannelLoop};
use crate::actor::delegate::DelegateActor;
use anyhow::{anyhow, Result};
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{Brief, Channel, Delegation, ThreadId};
use caelumora_domain::message::{Message, MessageId, MessageRole};
use caelumora_domain::store::Stored;
use caelumora_security::{ActionType, SecurityContext};
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tracing::{debug, warn};

impl ChannelLoop {
    /// Has the delegate of `thread` give its recipient `recipient` the brief `brief` to read on
    /// behalf of whom `context` acts for - its caller, the giver -, and bring what came of it back
    /// to them where the thread was opened; it waits behind a brief still open. Asked: that the
    /// giver reads the channel; that the thread is an open one opened at another; that the
    /// recipient runs in it and the giver where it was opened; that a thread delegated into
    /// before is delegated by the same giver to the same recipient; that the thread is not quiet
    /// after a cancel. The brief is classified as the channel is, and both directions answer the
    /// chain `context` belongs to (its initiator).
    ///
    /// # Errors
    /// `ChannelError::Forbidden`, `ThreadNotOpen`, `NotInThread`, `ThreadQuiet` or
    /// `DelegatedOtherwise`.
    pub async fn delegate(
        &self,
        context: &SecurityContext,
        thread: ThreadId,
        recipient: ActorId,
        brief: String,
    ) -> Result<()> {
        self.require(
            context,
            |attributes| ActionType::get(Channel::KIND, attributes),
            "delegating in",
        )
        .await?;
        let not_open = || ChannelError::ThreadNotOpen { thread, channel: self.id };
        let opening = self
            .thread(thread)
            .filter(|opened| opened.phase() == Phase::Open)
            .and_then(|opened| opened.opening())
            .ok_or_else(not_open)?;
        let giver = context.caller_id;
        let origin = opening.origin.thread;
        let runs_in = |participant: ActorId, thread: ThreadId| {
            let actors = self.actors.load();
            actors.get(&participant).is_some_and(|actor| actor.thread == thread)
        };
        let not_in = |participant, thread| ChannelError::NotInThread {
            participant,
            thread,
            channel: self.id,
        };
        // An agent: what comes of a brief is the turn that answers it, which a person has not.
        if !matches!(recipient, ActorId::Agent(_)) || !runs_in(recipient, thread) {
            return Err(not_in(recipient, thread).into());
        }
        if !runs_in(giver, origin) {
            return Err(not_in(giver, origin).into());
        }
        // Delegating does not wake a thread, so that a giver cannot undo a user's cancel.
        if self.is_quiet(thread) {
            return Err(ChannelError::ThreadQuiet { thread, channel: self.id }.into());
        }
        if !self.delegated_alike(thread, giver, recipient).await {
            return Err(ChannelError::DelegatedOtherwise { thread, channel: self.id }.into());
        }
        let message = Message {
            id: MessageId::new(),
            sender_id: giver,
            sender_name: self.name_of(giver).await,
            role: MessageRole::User,
            content: Some(brief),
            ..Message::new(self.classification())
        };
        let brief = Brief { message, given_at: None };
        let briefs = VecDeque::from([brief]);
        let delegation = Delegation { giver, recipient, initiator: context.initiator_id, briefs };
        let _ = self.request(ActorChange::Delegate(thread, delegation));
        debug!("{} delegates into thread {} of channel {}", giver, thread, self.id);
        Ok(())
    }

    /// What was delegated into `thread`, if it is still open and was delegated into (issue 557):
    /// its giver, its recipient and its briefs, answered or not, as the channel's state holds them
    /// now - what the work on a sub-agent asks whose it is.
    pub async fn delegation(&self, thread: ThreadId) -> Option<Delegation> {
        let state = self.state.lock().await;
        state.threads.get(&thread).filter(|held| !held.closed)?.delegation.clone()
    }

    /// Whether `giver` may delegate into `thread` to `recipient` (`ThreadState::delegates_alike`):
    /// a thread opened that `run` has not recorded yet is delegated into by nobody.
    async fn delegated_alike(&self, thread: ThreadId, giver: ActorId, recipient: ActorId) -> bool {
        let state = self.state.lock().await;
        state.threads.get(&thread).is_none_or(|held| held.delegates_alike(giver, recipient))
    }

    /// Records `delegation`'s briefs behind those of the delegation of `thread`, or as its
    /// delegation if it has none (`ThreadState::delegate`), and stores them; then has its delegate
    /// follow - starting it if it does not run, built from what was just stored. For `run` alone,
    /// which carries out what `delegate` asked, one change after the other. A thread no longer
    /// open, or delegated otherwise since, takes nothing: the brief is dropped, logged.
    pub(super) async fn hand_to_delegate(&self, thread: ThreadId, delegation: Delegation) {
        let held = {
            let mut state = self.state.lock().await;
            let open = self.is_open(thread);
            let mut taken = None;
            if let Some(held) = state.threads.get_mut(&thread).filter(|_| open) {
                if held.delegate(delegation) {
                    taken = held.delegation.clone();
                }
            }
            let Some(held) = taken else {
                warn!("A brief into thread {} of channel {} is dropped", thread, self.id);
                return;
            };
            if let Err(e) = self.store_state(&mut state).await {
                warn!("Cannot store the delegation into thread {}: {}", thread, e);
            }
            held
        };
        match self.delegate_of(thread) {
            Some(delegate) => delegate.sync(held),
            None => self.start_delegate(thread).await,
        }
    }

    /// Has the delegate of `thread`, if it runs, follow `delegation` as the channel's state holds
    /// it now (`DelegateActor::sync`).
    pub(super) fn sync_delegate(&self, thread: ThreadId, delegation: &Delegation) {
        if let Some(delegate) = self.delegate_of(thread) {
            delegate.sync(delegation.clone());
        }
    }

    /// Starts the delegates of the threads with briefs still to answer, as the channel starts -
    /// after its threads are built and what changed while it did not run is reconciled, before
    /// its participants: each reads on from where its open brief was given (`DelegateActor`).
    pub(super) async fn start_delegates(&self) {
        let delegated: Vec<ThreadId> = {
            let state = self.state.lock().await;
            state
                .threads
                .iter()
                .filter(|(_, held)| !held.closed)
                .filter(|(_, held)| held.delegation.as_ref().is_some_and(|d| !d.briefs.is_empty()))
                .map(|(thread, _)| *thread)
                .collect()
        };
        for thread in delegated {
            self.start_delegate(thread).await;
        }
    }

    /// Starts the delegate of `thread`, in it.
    async fn start_delegate(&self, thread: ThreadId) {
        let resolved = ResolvedParticipant::delegate(thread, self.session_id, self.id);
        if let Err(e) = self.start_participant(resolved, thread, None, None).await {
            warn!("The delegate of thread {} of channel {} cannot start: {:#}", thread, self.id, e);
        }
    }

    /// The delegate of `thread`, built from what the channel's state holds of its delegation, and
    /// where the thread was opened.
    ///
    /// # Errors
    /// If the state holds no delegation of `thread`, or `thread` is the line.
    pub(super) async fn build_delegate(&self, thread: ThreadId) -> Result<DelegateActor> {
        let state = self.state.lock().await;
        let held = state.threads.get(&thread);
        let origin = held.and_then(|held| held.opening).map(|opening| opening.origin.thread);
        let delegation = held.and_then(|held| held.delegation.clone());
        drop(state);
        let (Some(origin), Some(delegation)) = (origin, delegation) else {
            return Err(anyhow!("Thread {} of channel {} has no delegation", thread, self.id));
        };
        let name = self.name_of(delegation.recipient).await;
        let prompts = Arc::clone(self.runtime.prompts());
        let ids = (self.session_id, self.id);
        Ok(DelegateActor::new(thread, origin, ids.0, ids.1, delegation, name, prompts))
    }

    /// The delegate of `thread`, if it was taken in.
    fn delegate_of(&self, thread: ThreadId) -> Option<Arc<DelegateActor>> {
        let actor = self.actor(ActorId::Delegate(thread))?;
        let any_actor: Arc<dyn Any + Send + Sync> = actor;
        Arc::downcast::<DelegateActor>(any_actor).ok()
    }

    /// Lets the delegate of `thread` go, as its thread is closed: out of the channel as an actor
    /// is let go (`drop_actor`), its task out of `running` - which closes its inbox - and its
    /// delegation out of the channel's state, what was open dropped. An internal actor of the
    /// channel: nothing announced.
    pub(super) async fn let_delegate_go(
        &self,
        thread: ThreadId,
        running: &mut HashMap<ActorId, RunningActor>,
    ) {
        let delegate = ActorId::Delegate(thread);
        if let Some(actor) = self.actor(delegate) {
            self.drop_actor(delegate, &actor, thread);
        }
        running.remove(&delegate);
        if let Some(held) = self.state.lock().await.threads.get_mut(&thread) {
            held.delegation = None;
        }
    }
}
