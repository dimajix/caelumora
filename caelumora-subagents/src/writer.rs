//! The writer of sub-agents (issue 558), bound to the session of an agent's work: made of the
//! kernel's building blocks and procedures - an agent made (`AgentManager`), a thread founded with
//! it (`SessionLoop::open_thread`), a brief delegated into it (`ChannelLoop::delegate`), and the
//! agent let go (`procedure::participant::LetGo`, issue 593), its thread closed and the agent
//! deleted as its work ends (issue 557). It acts in the
//! work's context, each operation of the kernel checking itself: the agent of the work gives, its
//! owner owns what it starts.

use crate::extension::{SubAgentError, SubAgentResult, SubAgentWriter};
use crate::names;
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::reader::ViewError;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{AgentId, PersonalityId, PersonalityKind};
use caelumora_domain::channel::{ThreadId, ThreadOrigin};
use caelumora_domain::scope::Scope;
use caelumora_domain::store::StoreError;
use caelumora_domain::value::name::Name;
use caelumora_kernel::agent::{AgentError, AgentManager, AgentSpec};
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::error::{refusal, user_message};
use caelumora_kernel::procedure::participant::LetGo;
use caelumora_kernel::session::{SessionError, SessionLoop};
use caelumora_kernel::view::SessionView;
use caelumora_security::SecurityContext;
use std::future::Future;
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

    /// Starts an agent of `personality` on `brief` in a thread opened at `origin` (`start`),
    /// rolling back what was done if a step fails (`roll_back_start`).
    async fn start_now(
        &self,
        personality: Name,
        origin: ThreadOrigin,
        brief: String,
    ) -> SubAgentResult<ThreadId> {
        let (session, channel) = self.running().await?;
        let personality_id = self.personality(&personality).await?;
        let agent = self.make_agent(personality_id, &personality).await?;
        let context = self.context();
        let founder = ActorId::Agent(agent);
        let opened = session.open_thread(context, context.channel_id, origin, vec![founder]).await;
        let thread = match opened {
            Ok(thread) => thread,
            Err(error) => {
                self.roll_back_start(&session, &channel, agent, None).await;
                return Err(told(error));
            }
        };
        if let Err(error) = channel.delegate(context, thread, founder, brief).await {
            self.roll_back_start(&session, &channel, agent, Some(thread)).await;
            return Err(told(error));
        }
        info!("Sub-agent {} started in thread {} by {}", agent, thread, context.caller_id);
        Ok(thread)
    }

    /// Rolls back starting `agent`, once a step after making it failed: ended as its work ends, if
    /// its thread was opened (`wind_up`), deleted alone otherwise - in the work's name, whose owner
    /// made it. What cannot be rolled back is logged and stays.
    async fn roll_back_start(
        &self,
        session: &SessionLoop,
        channel: &ChannelLoop,
        agent: AgentId,
        thread: Option<ThreadId>,
    ) {
        let undone = match thread {
            Some(thread) => self.wind_up(session, channel, thread, agent).await,
            None => self.agents.delete(self.context(), agent).await.map(drop).map_err(told),
        };
        if let Err(undo) = undone {
            warn!("Sub-agent {} not started was not removed again: {}", agent, undo);
        }
    }

    /// Lets `agent`, the sub-agent of `thread`, go from the session, closes `thread` and deletes
    /// the agent - in this order, each only once the one before it is done, since the session lets
    /// no deleted agent go and closing waits for nobody to take part. One let go already - an end
    /// asked again after it stopped half-way - counts as let go.
    async fn wind_up(
        &self,
        session: &SessionLoop,
        channel: &ChannelLoop,
        thread: ThreadId,
        agent: AgentId,
    ) -> SubAgentResult<()> {
        let context = self.context();
        let sessions = session
            .sessions()
            .ok_or_else(|| SubAgentError::Failed("the kernel's sessions are gone".to_string()))?;
        let leaving = LetGo::leaving(&sessions, context, session.id(), ActorId::Agent(agent), None);
        match session.procedures().run(leaving).await {
            Ok(()) => {}
            Err(SessionError::NotAParticipant { .. }) => {}
            Err(error) => return Err(told(error.untyped())),
        }
        channel.close_thread(context, thread).await.map_err(told)?;
        self.agents.delete(context, agent).await.map_err(told)?;
        info!("Sub-agent {} of thread {} ended", agent, thread);
        Ok(())
    }

    /// Ends the sub-agent of `thread` (`end`).
    async fn end_now(&self, thread: ThreadId) -> SubAgentResult<()> {
        let (session, channel) = self.running().await?;
        let agent = self.sub_agent(&channel, thread).await?;
        self.wind_up(&session, &channel, thread, agent).await
    }
}

#[async_trait]
impl SubAgentWriter for SubAgents {
    /// Run apart, so that a turn cancelled or stopped while it runs leaves no sub-agent half
    /// started (TOOLS.md, *Running tool calls*).
    async fn start(
        &self,
        personality: &str,
        origin: ThreadOrigin,
        brief: String,
    ) -> SubAgentResult<ThreadId> {
        let unknown = || SubAgentError::UnknownPersonality(personality.to_string());
        let personality = Name::new(personality).map_err(|_| unknown())?;
        let this = self.clone();
        apart(async move { this.start_now(personality, origin, brief).await }).await
    }

    async fn ask(&self, thread: ThreadId, text: String) -> SubAgentResult<()> {
        let (_, channel) = self.running().await?;
        let agent = self.sub_agent(&channel, thread).await?;
        let context = self.context();
        channel.delegate(context, thread, ActorId::Agent(agent), text).await.map_err(told)
    }

    /// Run apart, as `start`, so that no sub-agent is left half ended.
    async fn end(&self, thread: ThreadId) -> SubAgentResult<()> {
        let this = self.clone();
        apart(async move { this.end_now(thread).await }).await
    }
}

/// Runs `work` in a task of its own and waits for it: dropping the wait leaves the work to finish.
async fn apart<T: Send + 'static>(
    work: impl Future<Output = SubAgentResult<T>> + Send + 'static,
) -> SubAgentResult<T> {
    tokio::spawn(work).await.map_err(|ended| {
        warn!("Sub-agents' work ended without an answer: {}", ended);
        SubAgentError::Failed("it ended without an answer; the log tells why".to_string())
    })?
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
