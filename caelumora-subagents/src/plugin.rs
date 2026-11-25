//! How sub-agents are joined to the kernel (issue 558): the `SubAgentWriter` bound to the session
//! of every piece of work of an agent that runs - what the component starts, asks and ends
//! sub-agents through (issue 516) -, within the limits of the plugin's section of the
//! configuration (`SubagentsConfig`, issue 597). It keeps nothing of its own: a sub-agent is an
//! agent, its thread and its delegation the channel's.

use crate::extension::SubAgentWriter;
use crate::writer::{SubAgents, SubagentsConfig};
use crate::SUBAGENTS;
use anyhow::Result;
use caelumora_kernel::agent::AgentManager;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use std::sync::Arc;

/// Sub-agents, as a plugin.
#[derive(Debug, Default)]
pub struct SubagentsPlugin;

impl Plugin for SubagentsPlugin {
    fn name(&self) -> &'static str {
        SUBAGENTS
    }

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        let limits: SubagentsConfig = kernel.config(SUBAGENTS)?;
        // Bound where the work's session runs - a sub-agent is started in a running channel -, with
        // the agents' manager the kernel provides once it is built.
        kernel.extend_writers::<dyn SubAgentWriter, _, _>(move |session| async move {
            let agents = Arc::clone(session.views().service::<AgentManager>()?);
            session.session_loop()?;
            Some(Arc::new(SubAgents::new(session, agents, limits)) as Arc<dyn SubAgentWriter>)
        })?;
        Ok(())
    }
}
