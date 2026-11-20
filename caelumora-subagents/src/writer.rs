//! The writer of sub-agents (issue 558), bound to the session of an agent's work: starting and
//! ending a sub-agent are procedures of its own (issue 593) - an agent made (`AgentManager`), a
//! thread founded with it (`procedure::thread::open`), a brief delegated into it
//! (`ChannelLoop::delegate`); the agent let go (`procedure::participant::let_go_from_session`), its
//! thread closed (`ChannelLoop::close_thread_held`) and the agent deleted as its work ends (issue
//! 557) -, made of the kernel's building blocks. It acts in the work's context, each operation of
//! the kernel checking itself: the agent of the work gives, its owner owns what it starts.

use crate::extension::{SubAgentError, SubAgentResult, SubAgentWriter};
use crate::names;
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::reader::ViewError;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{AgentId, PersonalityId, PersonalityKind};
use caelumora_domain::channel::{ChannelId, ThreadId, ThreadOrigin};
use caelumora_domain::scope::Scope;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreError;
use caelumora_domain::value::name::Name;
use caelumora_kernel::agent::{AgentError, AgentManager, AgentSpec};
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::error::{refusal, user_message};
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::procedure::participant::{self, lock_letting_go, Exit};
use caelumora_kernel::procedure::thread;
use caelumora_kernel::procedure::{
    Compensation, Deferred, Key, Locks, Procedure, ProcedureError, ProcedureExecutor,
};
use caelumora_kernel::session::{SessionError, SessionLoop};
use caelumora_kernel::view::SessionView;
use caelumora_security::SecurityContext;
use std::sync::Arc;
use tracing::{info, warn};

/// How often a name is drawn again when another agent of the channel's scope took it meanwhile.
const NAMINGS: usize = 3;

/// Sub-agents as the work bound to `session` starts, asks and ends them, with the agents'
/// manager the kernel provides.
#[derive(Clone)]
pub(crate) struct SubAgents {
    session: SessionView,
    agents: Arc<AgentManager>,
}

impl SubAgents {
    /// The writer of the work `session` was made of, its context the work's, making and deleting
    /// agents by `agents`.
    pub(crate) fn new(session: SessionView, agents: Arc<AgentManager>) -> Self {
        Self { session, agents }
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
    /// of an internal agent alone.
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

    /// Lets `agent`, the sub-agent of `thread`, go from the session and closes `thread`, for a
    /// procedure that holds them - in this order, since closing waits for nobody to take part. One
    /// let go already - an end asked again after it stopped half-way - counts as let go.
    async fn wind_up_held(
        &self,
        session: &SessionLoop,
        channel: &ChannelLoop,
        thread: ThreadId,
        agent: AgentId,
    ) -> SubAgentResult<()> {
        let context = self.context();
        let leaving = participant::let_go_from_session(
            session,
            context,
            ActorId::Agent(agent),
            Exit::Leaving,
        );
        match leaving.await {
            // Let go already, as its owner may take it out.
            Ok(()) | Err(SessionError::NotAParticipant { .. }) => {}
            Err(error) => return Err(told(error.untyped())),
        }
        channel.close_thread_held(context, thread).await.map_err(told)?;
        Ok(())
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

    async fn ask(&self, thread: ThreadId, text: String) -> SubAgentResult<()> {
        let (_, channel) = self.running().await?;
        let agent = self.sub_agent(&channel, thread).await?;
        let context = self.context();
        channel.delegate(context, thread, ActorId::Agent(agent), text).await.map_err(told)
    }

    /// One procedure, as `start`, so that no sub-agent is left half ended.
    async fn end(&self, thread: ThreadId) -> SubAgentResult<()> {
        let context = self.context();
        let (session, channel) = (context.session_id, context.channel_id);
        self.procedures()?.run(Ending { work: self.clone(), session, channel, thread }).await
    }
}

/// Starting a sub-agent (issue 558) as one procedure (issue 593): an agent of `personality` made, a
/// thread opened at `origin` with it as its founder (`procedure::thread::open`), `brief` delegated
/// into it - undone by its compensation, the last first, if a step fails: the agent taken out of
/// the channel and the session, its thread closed, the agent deleted. Holds the new thread, its id
/// chosen before, and the thread it is opened at; the agent it makes nobody knows before it is
/// made, so that it holds none of it.
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
        _: &Kernel,
        log: &mut Compensation,
    ) -> SubAgentResult<ThreadId> {
        let Self { work, personality, origin, brief, channel, thread, .. } = self;
        let (session, channel_loop) = work.running().await?;
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
        let founders = vec![founder];
        let opening = thread::open(log, &session, &context, channel, thread, origin, founders);
        opening.await.map_err(told)?;
        channel_loop.delegate(&context, thread, founder, brief).await.map_err(told)?;
        info!("Sub-agent {} started in thread {} by {}", agent, thread, context.caller_id);
        Ok(thread)
    }
}

/// Ending a sub-agent (issue 558) as one procedure (issue 593): let go from the session, its
/// thread closed, the agent deleted - in this order, each only once the one before it is done,
/// since the session lets no deleted agent go and closing waits for nobody to take part. Holds
/// what letting the agent go needs (`procedure::participant::lock_letting_go`), and the thread
/// exclusively.
struct Ending {
    work: SubAgents,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
}

#[async_trait]
impl Procedure for Ending {
    /// The sub-agent of the thread, as read in the kernel's name, or why there is none.
    type Plan = SubAgentResult<AgentId>;
    type Output = ();
    type Error = SubAgentError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<Self::Plan, Deferred> {
        let Some(session) = self.work.session.session_loop() else {
            return Ok(Err(SubAgentError::Failed("its session does not run".to_string())));
        };
        let Some(channel) = session.channel(self.channel) else {
            return Ok(Err(SubAgentError::NoSubAgent(self.thread)));
        };
        let agent = match self.work.sub_agent(&channel, self.thread).await {
            Ok(agent) => agent,
            Err(error) => return Ok(Err(error)),
        };
        // As letting the agent go needs it, and its thread to close exclusively.
        let participant = ActorId::Agent(agent);
        lock_letting_go(locks, Some(session), self.session, participant, None).await?;
        locks.exclusive(Key::Thread(self.session, self.channel, self.thread))?;
        Ok(Ok(agent))
    }

    /// The sub-agent asked again, holding it: one ended meanwhile by another end is none.
    async fn carry_out(
        self,
        agent: Self::Plan,
        _: &Kernel,
        _: &mut Compensation,
    ) -> SubAgentResult<()> {
        agent?;
        let Self { work, thread, .. } = self;
        let (session, channel) = work.running().await?;
        let agent = work.sub_agent(&channel, thread).await?;
        work.wind_up_held(&session, &channel, thread, agent).await?;
        work.agents.delete(work.context(), agent).await.map_err(told)?;
        info!("Sub-agent {} of thread {} ended", agent, thread);
        Ok(())
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
