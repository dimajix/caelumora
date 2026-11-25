//! The writer of sub-agents (issue 558), bound to the session of an agent's work: starting a
//! sub-agent is a procedure of its own (issue 593) - an agent made (`AgentManager`), a thread
//! founded with it that owns it (`procedure::thread::open`, issue 598), a brief delegated into it
//! (`ChannelLoop::delegate`) -; ending it is closing its thread, with the agent it owns
//! (`procedure::thread::Close`) - made of the kernel's building blocks. It acts in the work's
//! context, each operation of the kernel checking itself: the agent of the work gives, its owner
//! owns what it starts; within the limits the operator set (`SubagentsConfig`, issue 597).

use crate::extension::{SubAgentError, SubAgentResult, SubAgentWriter};
use crate::names;
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::reader::ViewError;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{AgentId, PersonalityId, PersonalityKind};
use caelumora_domain::channel::{ChannelId, Circle, ThreadId, ThreadOrigin};
use caelumora_domain::scope::Scope;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreError;
use caelumora_domain::value::name::Name;
use caelumora_kernel::agent::{AgentError, AgentManager, AgentSpec};
use caelumora_kernel::channel::{ChannelError, ChannelLoop, ThreadSpec};
use caelumora_kernel::error::{refusal, user_message};
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::procedure::thread;
use caelumora_kernel::procedure::{
    Compensation, Deferred, Key, Locks, Procedure, ProcedureError, ProcedureExecutor,
};
use caelumora_kernel::session::{SessionError, SessionLoop};
use caelumora_kernel::view::SessionView;
use caelumora_security::SecurityContext;
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};

/// How often a name is drawn again when another agent of the channel's scope took it meanwhile.
const NAMINGS: usize = 3;

/// The plugin's section of the configuration (`plugins.subagents`, issue 597): how many sub-agents
/// may be open at once in a tree - an agent and all that descend from it -, and how deep they may
/// lie - 1 for sub-agents that start none of their own. Set by the operator and checked by the
/// writer, on the kernel's side: a component's parameters are the agent side's word.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SubagentsConfig {
    pub(crate) max_open: usize,
    pub(crate) max_depth: usize,
}

impl Default for SubagentsConfig {
    fn default() -> Self {
        Self { max_open: 5, max_depth: 1 }
    }
}

/// Sub-agents as the work bound to `session` starts, asks and ends them, with the agents'
/// manager the kernel provides, within `limits`.
#[derive(Clone)]
pub(crate) struct SubAgents {
    session: SessionView,
    agents: Arc<AgentManager>,
    limits: SubagentsConfig,
}

impl SubAgents {
    /// The writer of the work `session` was made of, its context the work's, making and deleting
    /// agents by `agents`, starting them within `limits`.
    pub(crate) fn new(
        session: SessionView,
        agents: Arc<AgentManager>,
        limits: SubagentsConfig,
    ) -> Self {
        Self { session, agents, limits }
    }

    fn context(&self) -> &SecurityContext {
        self.session.views().context()
    }

    /// The running session and the work's channel in it, reached as a frontend reaches a channel
    /// (`SessionLoop::channel_for`), which asks whether the work may.
    async fn running(&self) -> SubAgentResult<(Arc<SessionLoop>, Arc<ChannelLoop>)> {
        let no_session = || SubAgentError::Failed("its session does not run".to_string());
        let session = self.session.session_loop().ok_or_else(no_session)?;
        let context = self.context();
        let channel = session.channel_for(context, context.channel_id).await.map_err(told)?;
        Ok((Arc::clone(session), channel))
    }

    /// The personality `personality` an agent may be made of in the work's channel: the one the
    /// owner finds from there, as the kernel finds one by name (`Views::personality_named`) - one
    /// of an internal agent alone, whose agents are cleared no wider than the work's agent: limited
    /// at least as the work's context is (`SecurityContext::covers_limit`, issue 597).
    async fn personality(&self, personality: &Name) -> SubAgentResult<PersonalityId> {
        let place = self.scope();
        let found = match self.session.views().personality_named(personality, &place).await {
            Ok(found) => found,
            Err(ViewError::NoPersonalityNamed(_)) => {
                return Err(SubAgentError::UnknownPersonality(personality.to_string()))
            }
            Err(error) => return Err(told(error)),
        };
        if found.kind == PersonalityKind::External {
            return Err(SubAgentError::External(personality.to_string()));
        }
        if !self.context().covers_limit(found.limiting_roles()) {
            return Err(SubAgentError::ClearedWider(personality.to_string()));
        }
        Ok(found.id)
    }

    /// The scope of the work's channel: where a sub-agent lies, and where its personality is found
    /// from.
    fn scope(&self) -> Scope {
        let context = self.context();
        Scope::ChannelScope { session_id: context.session_id, channel_id: context.channel_id }
    }

    /// The names the work's channel has had, among its participants and its former ones.
    async fn names_had(&self) -> SubAgentResult<Vec<Name>> {
        let channel = self.session.channel(self.context().channel_id).await.map_err(told)?;
        let participants = channel.participants().await;
        let former = channel.former_participants().await;
        Ok(participants.iter().chain(&former).filter_map(|p| p.name().cloned()).collect())
    }

    /// An agent of the personality `personality_id`, named `personality`, made by the work's owner
    /// in the channel's scope under a name the channel has never had - drawn again if another agent
    /// of that scope took it meanwhile.
    async fn make_agent(
        &self,
        personality_id: PersonalityId,
        personality: &Name,
    ) -> SubAgentResult<AgentId> {
        let context = self.context();
        let scope = self.scope();
        let mut had = self.names_had().await?;
        for _ in 0..NAMINGS {
            let name = names::draw(personality, |name| had.contains(name));
            let spec = AgentSpec::new(name.to_string(), scope.clone(), personality_id);
            match self.agents.create(context, spec).await {
                Ok(agent) => return Ok(agent.id),
                Err(AgentError::Store(StoreError::NameTaken { .. })) => had.push(name),
                Err(error) => return Err(told(error)),
            }
        }
        Err(SubAgentError::Failed("no name for the sub-agent was free".to_string()))
    }

    /// The sub-agent of `thread` the work started: the recipient of the thread's delegation, whose
    /// giver is the work's agent.
    async fn sub_agent(&self, channel: &ChannelLoop, thread: ThreadId) -> SubAgentResult<AgentId> {
        let delegation = channel.delegation(thread).await;
        delegation
            .filter(|held| held.giver == self.context().caller_id)
            .and_then(|held| held.recipient.as_agent_id())
            .ok_or(SubAgentError::NoSubAgent(thread))
    }

    /// The executor of the kernel, reached through the session the work is bound to, which
    /// carries out the procedures of the sub-agents.
    fn procedures(&self) -> SubAgentResult<&Arc<ProcedureExecutor>> {
        let no_session = || SubAgentError::Failed("its session does not run".to_string());
        Ok(self.session.session_loop().ok_or_else(no_session)?.procedures())
    }
}

#[async_trait]
impl SubAgentWriter for SubAgents {
    /// One procedure, carried out in a task of the executor's, so that a turn cancelled or stopped
    /// while it runs leaves no sub-agent half started (TOOLS.md, *Running tool calls*).
    async fn start(
        &self,
        personality: &str,
        origin: ThreadOrigin,
        brief: String,
    ) -> SubAgentResult<ThreadId> {
        let unknown = || SubAgentError::UnknownPersonality(personality.to_string());
        let personality = Name::new(personality).map_err(|_| unknown())?;
        let context = self.context();
        let starting = Starting {
            work: self.clone(),
            personality,
            origin,
            brief,
            session: context.session_id,
            channel: context.channel_id,
            thread: ThreadId::new(),
        };
        self.procedures()?.run(starting).await
    }

    /// One whose delegation was let go (issue 598) - the work's agent left it without a word in
    /// the turn that read its answer - is none.
    async fn ask(&self, thread: ThreadId, text: String) -> SubAgentResult<()> {
        let (_, channel) = self.running().await?;
        let agent = self.sub_agent(&channel, thread).await?;
        let context = self.context();
        let asking = channel.delegate(context, thread, ActorId::Agent(agent), text).await;
        asking.map_err(|error| match error.downcast_ref::<ChannelError>() {
            Some(ChannelError::DelegationLetGo { .. }) => SubAgentError::NoSubAgent(thread),
            _ => told(error),
        })
    }

    /// Closing its thread, with the agent it owns (`procedure::thread::Close`), one procedure as
    /// `start`, so that no sub-agent is left half ended; one closed meanwhile by another end is
    /// none.
    async fn end(&self, thread: ThreadId) -> SubAgentResult<()> {
        let (_, channel) = self.running().await?;
        let agent = self.sub_agent(&channel, thread).await?;
        let context = self.context();
        let closing = thread::Close::new(context, context.session_id, context.channel_id, thread);
        let closed = self.procedures()?.run(closing).await.map_err(|e| told(e.untyped()))?;
        if !closed {
            return Err(SubAgentError::NoSubAgent(thread));
        }
        info!("Sub-agent {} of thread {} ended", agent, thread);
        Ok(())
    }
}

/// Starting a sub-agent (issue 558) as one procedure (issue 593): an agent of `personality` made, a
/// thread opened at `origin` with it as its founder, which owns it (`procedure::thread::open`,
/// issue 598), `brief` delegated into it - undone by its compensation, the last first, if a step
/// fails: the agent taken out of the channel and the session, its thread closed, the agent
/// deleted. Within the limits (issue 597): refused before anything is made if the work is a
/// sub-agent as deep as allowed, or as many sub-agents are open in the work's tree as allowed; and
/// once its thread is opened, if more are open than allowed - counted with its own, so that two
/// started at once never pass the limit, at worst both refused. Holds the new thread, its id chosen
/// before, and the thread it is opened at; the agent it makes nobody knows before it is made, so
/// that it holds none of it.
struct Starting {
    work: SubAgents,
    personality: Name,
    origin: ThreadOrigin,
    brief: String,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
}

#[async_trait]
impl Procedure for Starting {
    type Plan = ();
    type Output = ThreadId;
    type Error = SubAgentError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        locks.exclusive(Key::Thread(self.session, self.channel, self.thread))?;
        locks.shared(Key::Thread(self.session, self.channel, self.origin.thread))
    }

    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> SubAgentResult<ThreadId> {
        let Self { work, personality, origin, brief, channel, thread, .. } = self;
        let (session, channel_loop) = work.running().await?;
        let SubagentsConfig { max_open, max_depth } = work.limits;
        let giver = work.context().caller_id;
        if channel_loop.ownership_depth(giver).await >= max_depth {
            return Err(SubAgentError::TooDeep(max_depth));
        }
        // Refused before anything is made where the tree is full already; counted again once the
        // thread is opened, for one started meanwhile.
        if channel_loop.owned_in_tree(giver).await >= max_open {
            return Err(SubAgentError::TooMany(max_open));
        }
        let personality_id = work.personality(&personality).await?;
        let agent = work.make_agent(personality_id, &personality).await?;
        let founder = ActorId::Agent(agent);
        {
            let work = work.clone();
            log.record("making a sub-agent", async move {
                work.agents.delete(work.context(), agent).await?;
                Ok(())
            });
        }
        let context = work.context().clone();
        // Its founder's taking in and the opening, each taken back by `log` (`thread::open`).
        // A closed circle: nobody but the sub-agent takes part in it (issue 602); it owns the
        // sub-agent, which ends with it (issue 598).
        let spec = ThreadSpec {
            circle: Circle::Closed,
            founders: vec![founder],
            owns_founders: true,
            ..ThreadSpec::new(origin)
        };
        let opening = thread::open(log, kernel, &session, &context, channel, thread, &spec);
        opening.await.map_err(told)?;
        // Recorded by the channel once `open` returns, which waits for it to draw the author in.
        if channel_loop.owned_in_tree(giver).await > max_open {
            return Err(SubAgentError::TooMany(max_open));
        }
        channel_loop.delegate(&context, thread, founder, brief).await.map_err(told)?;
        info!("Sub-agent {} started in thread {} by {}", agent, thread, context.caller_id);
        Ok(thread)
    }
}

/// What the executor fails with instead of carrying a procedure of the sub-agents out, as a
/// sub-agents' error, as the kernel tells it (`told`).
impl From<ProcedureError> for SubAgentError {
    fn from(error: ProcedureError) -> Self {
        told(SessionError::from(error))
    }
}

/// What the kernel refused the work or failed with, `error`, as a sub-agents' error: a refusal of
/// the access policy as `Forbidden`, saying what was refused and not by which rule (`Refused`);
/// anything else as `Failed`, as users are told it (`user_message`), its whole chain logged.
fn told(error: impl Into<Error>) -> SubAgentError {
    let error = error.into();
    if let Some(refused) = refusal(&error) {
        info!("Sub-agents were refused: {}", refused);
        return SubAgentError::Forbidden(refused.to_string());
    }
    warn!("Sub-agents failed: {:#}", error);
    let told = user_message(&error);
    SubAgentError::Failed(told.unwrap_or_else(|| "it failed; the log tells why".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::security::Refused;

    /// Issue 558: what the access policy refused comes back as a refusal, saying what was refused
    /// and not by which rule; anything else as a failure, as users are told it.
    #[test]
    fn a_refusal_is_told_from_a_failure() {
        let refused = Refused { subject: "agent", action: "CreateAgent".to_string() };
        let told_refused = told(AgentError::Store(StoreError::PermissionDenied(refused)));
        assert!(
            matches!(&told_refused, SubAgentError::Forbidden(text)
                if text == "CreateAgent is not permitted"),
            "{told_refused}"
        );
        let broken = told(AgentError::Store(StoreError::Backend("the disk is full".to_string())));
        assert!(matches!(broken, SubAgentError::Failed(_)), "{broken}");
    }
}
