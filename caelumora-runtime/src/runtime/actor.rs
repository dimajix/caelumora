//! `ParticipantRuntime`: what the actor of a participant runs with in its channel (issue 69),
//! made by the core of the participant, and asking the core in its name.

use super::{ChannelRuntime, RuntimeManager};
use crate::actor::ActorManager;
use crate::attachment::AttachmentManager;
use crate::channel::{ChannelLoop, ResolvedParticipant};
use crate::command::CommandExecutor;
use crate::controller::{ChannelController, SessionController};
use crate::event::EventSubscription;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::security::{AccessPolicy, SecurityAttributes, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::store::StoreResult;
use std::sync::Arc;

/// What a participant's actor runs with (issue 69): the context it acts in, built by the core at
/// every call from what it holds of the participant (`acting_for`, issue 516), and access to what
/// concerns this actor alone - the controllers of its session and its channel, its channel's
/// events, the access policy and the managers the core's actors use. It is made for one participant
/// (`ChannelRuntime::participant_runtime`) and hands out nothing about others. The actors of the
/// core - the user's and the approver's - hold it; an agent's actor, on the agent side, is given
/// the `ActorRuntime` of its agent (`AgentRuntime`), which answers what only an agent asks - its
/// state, its session to read, what the core decides of its work - and asks this runtime for the
/// rest. What is the core's alone - the controllers, the commands of the session, the channel's
/// loop - is `pub(crate)`.
#[derive(Debug, Clone)]
pub struct ParticipantRuntime {
    pub(super) runtime: ChannelRuntime,
    pub(super) actor: ActorId,
    pub(super) session_id: SessionId,
    pub(super) channel_id: ChannelId,
    /// In whose name it acts, which the core builds into a context at every call (issue 516).
    pub(super) acting: Acting,
}

/// How the context an actor's runtime acts in is built, at every call that asks the core
/// (`ParticipantRuntime::acting_for`, issue 516).
#[derive(Debug, Clone)]
pub(crate) enum Acting {
    /// A participant of the channel, as the channel resolved it: its context as the controllers
    /// hold it now - a user's own, an agent's with its owner's rights and roles, limited by its
    /// personality, the approver's answered for by the channel's owner.
    Participant(ResolvedParticipant),
    /// In a test, an actor that takes part in nothing: in its own name, without roles.
    #[cfg(any(test, feature = "test-support"))]
    Itself,
    /// In a test, the context it gives (`ParticipantRuntime::acting_as`).
    #[cfg(any(test, feature = "test-support"))]
    Fixed(SecurityContext),
}

impl ParticipantRuntime {
    /// The actor this runtime is for.
    pub(crate) fn actor_id(&self) -> ActorId {
        self.actor
    }

    /// The context the actor acts in now, for `initiator` - who began the chain it answers, for
    /// the record, never for rights (issue 516): built by the core at every call, from what it
    /// holds of the actor (`Acting`), never handed in by the actor.
    pub(crate) fn acting_for(&self, initiator: ActorId) -> SecurityContext {
        match &self.acting {
            Acting::Participant(participant) => participant.context_in(&self.channel(), initiator),
            #[cfg(any(test, feature = "test-support"))]
            Acting::Itself => SecurityContext {
                initiator_id: initiator,
                ..SecurityContext::new(self.actor, self.session_id, self.channel_id)
            },
            #[cfg(any(test, feature = "test-support"))]
            Acting::Fixed(context) => context.clone(),
        }
    }

    /// This runtime, acting in `context` whatever its actor and its initiator are: for a test
    /// that gives the work of an actor the rights it needs without making it a participant
    /// (issue 516).
    #[cfg(any(test, feature = "test-support"))]
    pub fn acting_as(self, context: SecurityContext) -> Self {
        Self { acting: Acting::Fixed(context), ..self }
    }

    /// The runtime of the process the participant runs in, which the runtime of its agent answers
    /// by (`AgentRuntime`).
    pub(super) fn runtime_manager(&self) -> &Arc<RuntimeManager> {
        &self.runtime.runtime_manager
    }

    /// The access policy of the process - the core's alone, as its other policy and its secrets
    /// are: what an actor of the agent side needs decided it asks its `ActorRuntime` (issue 516).
    pub(crate) fn access_policy(&self) -> &Arc<dyn AccessPolicy> {
        self.runtime.runtime_manager.access_policy()
    }

    /// Where the actor's outbox is kept across its runs (issue 386).
    pub(crate) fn actor_manager(&self) -> &Arc<ActorManager> {
        self.runtime.runtime_manager.actor_manager()
    }

    /// The owner of attachments (issue 282): what a user's message names of what they uploaded,
    /// what a model is given along.
    pub(crate) fn attachment_manager(&self) -> &Arc<AttachmentManager> {
        self.runtime.runtime_manager.attachment_manager()
    }

    /// The controller of the actor's session: what its agent's tool calls
    /// are guarded by, beside the channel's (issue 13). Held for as long as
    /// the actor runs, unlike its channel: the guard must not lapse.
    pub(crate) fn session_controller(&self) -> &Arc<SessionController> {
        self.runtime.session_controller()
    }

    /// The controller of the actor's channel (issue 374), held as the
    /// session's is: what the actor reads its channel by, and what its
    /// agent's tool calls are guarded by.
    pub(crate) fn channel_controller(&self) -> &Arc<ChannelController> {
        self.runtime.channel_controller()
    }

    /// The user commands of the actor's session. Only a `UserActor` runs
    /// them: commands are for people, agents act through tools.
    pub(crate) fn command_executor(&self) -> &Arc<CommandExecutor> {
        &self.runtime.command_executor
    }

    /// The attributes of the agent `id`, as stored - what is asked of an
    /// agent named in an event (`ControllerManager::agent_attributes`).
    ///
    /// # Errors
    /// `StoreError::NotFound` if the agent no longer exists, or whatever
    /// else the store reports.
    pub(crate) async fn agent_attributes(&self, id: AgentId) -> StoreResult<SecurityAttributes> {
        self.runtime.runtime_manager.controller_manager().agent_attributes(id).await
    }

    /// The global events about the actor's channel (issue 69): nothing about
    /// other channels can be subscribed to. Changes to the actor's own user
    /// or agent are the session's to take in, into the controller the actor
    /// shares (issue 120).
    pub(crate) fn subscribe_channel(&self) -> EventSubscription {
        self.runtime.runtime_manager.event_bus().subscribe_channel(self.channel_id)
    }

    /// The actor's channel as it stands, read from its controller - as it stood last once the
    /// channel is gone (issue 374): what its classification is read from, and what an operation in
    /// the channel is given as the channel's properties (`AttachmentManager::copy`).
    pub(crate) fn channel(&self) -> Arc<Channel> {
        Arc::clone(&self.channel_controller().load())
    }

    /// The channel the actor is in, or `None` while the channel is still being built or once it
    /// is gone - for the user's and the approver's actors, which act for a person as the frontends
    /// do, for the views, and for the runtime of an agent, which answers what the agent asks of
    /// its channel (`AgentRuntime`, issue 370): the loop's public API acts in a user's name too.
    pub(crate) fn channel_loop(&self) -> Option<Arc<ChannelLoop>> {
        self.runtime.channel_loop.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{participant_runtime_in, Script, ScriptedAgents, World, STAFF};

    /// Decision 3 of issue 516: an agent's runtime acts with its owner's rights as they are at
    /// each call, not as they were when it was made - a role taken from her between two calls of
    /// one piece of work is gone for the second.
    #[tokio::test]
    async fn an_agent_s_runtime_acts_with_its_owner_s_roles_as_they_are_at_each_call() {
        let world = World::with(&[&ScriptedAgents(Script::Hangs)], |_| {}).await;
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let runtime = participant_runtime_in(&main, ActorId::Agent(helper));
        let bob = ActorId::User(world.bob);

        let context = runtime.acting_for(bob);
        assert_eq!(
            (context.caller_id, context.principal_id, context.initiator_id),
            (ActorId::Agent(helper), ActorId::User(world.alice), bob),
            "as its owner's agent, for who asked"
        );
        assert_eq!(context.roles, [*STAFF], "with her roles");
        world.take_roles(world.alice).await;
        assert!(runtime.acting_for(bob).roles.is_empty(), "not any more, at the next call");
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}
