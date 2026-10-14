//! The commands an agent carries out itself (issue 269), the same for both
//! kinds of agent - the `AgentActor` and the `ExternalAgentActor`: which it
//! has, who may ask for them, and carrying one out in a turn of its own. Its
//! body's (`AgentBody`).

use super::body::AgentBody;
use super::Answering;
use crate::command::{AgentCommandProvider, CommandError, CommandExecutor};
use crate::component::{ComponentContainer, ComponentError, ComponentResult};
use crate::runtime::ActorRuntime;
use crate::transport::EventSender;
use caelumora_model::actor::{ActorEvent, Envelope};
use caelumora_model::agent::AgentChannelState;
use caelumora_model::command::{Command, CommandVisibility};
use caelumora_model::security::SecurityContext;
use caelumora_model::turn::CommandRequest;
use std::sync::Arc;
use tracing::{debug, info, warn};

impl AgentBody {
    /// The commands the components of `components` provide (issue 269),
    /// behind the executor that checks who may see and ask for them.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two components provide the same
    /// name, or one provides a name the session's commands have: the
    /// session's would hide it (issue 37).
    pub(super) fn command_executor(components: &ComponentContainer, runtime: &ActorRuntime) -> ComponentResult<CommandExecutor<dyn AgentCommandProvider>> {
        let providers = components.get_all::<dyn AgentCommandProvider>(None)?;
        for provider in &providers {
            if let Some(command) = provider.commands().into_iter().find(|c| runtime.command_executor().has(&c.name)) {
                return Err(ComponentError::InvalidConfig(format!(
                    "command /{} of component '{}' is a command of the session too, which would hide it",
                    command.name, provider.id
                )));
            }
        }
        CommandExecutor::new(Arc::clone(runtime.security_manager()), providers)
    }

    /// The commands `issuer` may see of the agent in its channel, sorted by
    /// name; none without a channel.
    pub(super) async fn visible_commands(&self, issuer: &SecurityContext) -> Vec<Command> {
        let Some(channel) = self.runtime.channel_loop() else { return Vec::new() };
        let agent = self.agent.load().security_attributes.clone();
        self.commands.visible(issuer, &agent, &channel.security_attributes()).await
    }

    /// Carries out the `CommandRequested` in `request` in a turn of its own,
    /// begun on `state` in the request's contexts continued in the agent's
    /// name (`Answering::to`): its owner's rights, whoever asked its
    /// initiator. The executor asks, in the context the request came with,
    /// whether the issuer may ask for it - the one place that is asked
    /// (issue 304); admitted, the agent tells the issuer alone that it began
    /// (`CommandStarted`, issue 299), refused it answers `CommandFailed`.
    ///
    /// Carried out, the turn is taken over - what the command staged applied,
    /// the command recorded before the turn's end - and stored, and the
    /// agent answers `CommandExecuted`, to the issuer alone for a private
    /// answer. Not carried out, the turn is dropped as if it had never begun:
    /// nothing was said or applied, so nothing is recorded, and the agent
    /// answers `CommandFailed` to the issuer. A command turn is no answer:
    /// it announces no answer's start nor end, asks nobody to respond and
    /// is never asked for again. Where the agent stands meanwhile is its
    /// actor's to tell.
    pub(super) async fn carry_out(&self, state: &mut AgentChannelState, request: &Envelope, output: &dyn EventSender) {
        let ActorEvent::CommandRequested { command, args, .. } = &request.event else { return };
        let issuer = request.context.sender_id;
        let (agent_id, name) = (self.agent_id(), self.agent.name());
        let answering = Answering::to(request, self.id(), name.clone(), &self.security_context());
        let Some(channel) = self.runtime.channel_loop() else {
            warn!("Agent {} has no channel to carry out /{} in", agent_id, command);
            return;
        };
        let (attributes, channel_attributes) = (self.agent.load().security_attributes.clone(), channel.security_attributes());
        let kept = CommandRequest { command: command.clone(), args: args.clone(), issuer };
        let turn_id = state.begin_command_turn(
            answering.event_context.clone(),
            answering.security_context.clone(),
            channel.classification(),
            self.profile,
            kept,
        );
        let Some(mut turn) = self.turn(state) else {
            warn!("Agent {} lost the turn it began for /{}", agent_id, command);
            return;
        };
        let mut started = false;
        let carried_out = match self.commands.admitted(command, &request.security, &attributes, &channel_attributes).await {
            Ok(admitted) => {
                let start = ActorEvent::CommandStarted { command: command.clone(), args: args.clone(), to: issuer, turn_id };
                if let Err(e) = answering.send(start, output).await {
                    warn!("{:#}", e);
                }
                started = true;
                admitted.execute(args, &mut turn).await
            }
            Err(e) => Err(e),
        };
        let event = match carried_out {
            Ok(answer) => {
                let committed = turn.commit(false, output).await;
                self.store(state).await;
                if committed.channel_state_changed {
                    self.runtime.announce_channel_state_changed();
                }
                info!("Agent {} {} carried out /{} for {}", agent_id, name, command, issuer);
                let to = (answer.visibility == CommandVisibility::Private).then_some(issuer);
                ActorEvent::CommandExecuted { command: command.clone(), args: args.clone(), output: answer.text, to, turn_id: Some(turn_id) }
            }
            Err(e) => {
                drop(turn);
                // Never begun, as far as anything shows: nothing was said or
                // applied, so the turn leaves no end behind.
                state.turn = None;
                match &e {
                    CommandError::Failed { .. } => warn!("Agent {} {} could not carry out /{} for {}: {}", agent_id, name, command, issuer, e),
                    _ => debug!("Agent {} {} does not carry out /{} for {}: {}", agent_id, name, command, issuer, e),
                }
                ActorEvent::CommandFailed { command: command.clone(), reason: e.to_string(), to: issuer, turn_id: started.then_some(turn_id) }
            }
        };
        if let Err(e) = answering.send(event, output).await {
            warn!("{:#}", e);
        }
    }
}
