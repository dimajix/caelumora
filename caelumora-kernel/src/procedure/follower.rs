//! The follower of the agents' deletion and restoring (issue 593): an entry point that has
//! the running channels follow whether their agents exist (`participant::Follow`), which every
//! program that runs sessions starts beside its `SessionManager` - without it, a deleted agent
//! keeps running in the channels of a running session until the session starts again.

use super::participant::Follow;
use crate::session::SessionManager;
use caelumora_domain::agent::Agent;
use caelumora_domain::event::EntityChange;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

/// Follows, for the whole kernel, whether the agents of the running sessions exist: carries out a
/// `Follow` for every agent deleted or restored, each in a task of its own - so that it misses no
/// event while one waits -, and one for every running session after it lost events. Stops
/// receiving when dropped; a `Follow` under way ends by itself.
pub struct AgentFollower {
    _following: AbortOnDropHandle<()>,
}

impl AgentFollower {
    /// Follows the agents of the sessions `sessions` runs, from now on.
    pub fn spawn(sessions: &Arc<SessionManager>) -> Self {
        let sessions = Arc::clone(sessions);
        let procedures = Arc::clone(sessions.kernel().procedures());
        let mut events = sessions.kernel().event_bus().subscribe_all().into_receiver();
        let following = tokio::spawn(async move {
            loop {
                let follows = match events.recv().await {
                    Ok(event) => match event.about::<Agent>() {
                        Some((agent, EntityChange::Deleted | EntityChange::Restored)) => {
                            vec![Follow::agent(&sessions, agent)]
                        }
                        _ => continue,
                    },
                    Err(RecvError::Lagged(lost)) => {
                        warn!(
                            "The agent follower lost {lost} events and follows every session again"
                        );
                        let ids = sessions.running_ids();
                        ids.into_iter().map(|id| Follow::session(&sessions, id)).collect()
                    }
                    // Not while the kernel holds the bus.
                    Err(RecvError::Closed) => return,
                };
                for follow in follows {
                    let procedures = Arc::clone(&procedures);
                    tokio::spawn(async move {
                        if let Err(error) = procedures.run(follow).await {
                            warn!("An agent could not be followed: {error:#}");
                        }
                    });
                }
            }
        });
        Self { _following: AbortOnDropHandle::new(following) }
    }
}
