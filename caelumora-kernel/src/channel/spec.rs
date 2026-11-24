//! What a caller chooses of a new thread of a channel: `ThreadSpec`, taken by
//! `procedure::thread::Open` and `procedure::thread::open`, which take its founders in, and by
//! `ChannelLoop::open_thread`, which opens it at its origin with its circle.

use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::channel::{Circle, ThreadOrigin};

/// What a new thread should be (issues 557, 602, 598), as it was asked: where it hangs, whether who
/// takes part in it may grow, whom it is founded with, and whether it owns them.
#[derive(Debug, Clone)]
pub struct ThreadSpec {
    pub origin: ThreadOrigin,
    /// Whether who takes part in it may grow; open unless asked otherwise.
    pub circle: Circle,
    /// Whom it is founded with - whom its opener acts for, or their own agents -; nobody unless
    /// named.
    pub founders: Vec<ActorId>,
    /// Whether the agents it is founded with were made for it alone, and end with it as it is
    /// closed (issue 598, `Opening::owned`); not unless asked.
    pub owns_founders: bool,
}

impl ThreadSpec {
    /// A thread at `origin`, its circle open, founded with nobody.
    pub fn new(origin: ThreadOrigin) -> Self {
        Self { origin, circle: Circle::Open, founders: Vec::new(), owns_founders: false }
    }

    /// The agents it owns as it is opened: those it is founded with, each once, if it owns them.
    pub(crate) fn owned(&self) -> Vec<AgentId> {
        let mut owned: Vec<AgentId> = Vec::new();
        if self.owns_founders {
            for agent in self.founders.iter().filter_map(|founder| founder.as_agent_id()) {
                if !owned.contains(&agent) {
                    owned.push(agent);
                }
            }
        }
        owned
    }
}

#[cfg(test)]
mod tests {
    use super::ThreadSpec;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::channel::{ThreadId, ThreadOrigin};
    use caelumora_domain::message::MessageId;
    use caelumora_domain::user::UserId;

    /// Issue 598: a thread owns the agents it is founded with, each once, if asked - never a
    /// person.
    #[test]
    fn a_thread_owns_the_agents_it_is_founded_with_if_asked() {
        let origin = ThreadOrigin { thread: ThreadId::new(), message: MessageId::new() };
        let agent = AgentId::new();
        let founders =
            vec![ActorId::User(UserId::new()), ActorId::Agent(agent), ActorId::Agent(agent)];
        let spec = ThreadSpec { founders, ..ThreadSpec::new(origin) };
        assert!(spec.owned().is_empty(), "not unless asked");
        let owning = ThreadSpec { owns_founders: true, ..spec };
        assert_eq!(owning.owned(), [agent], "its agent, once, and not the person");
    }
}
