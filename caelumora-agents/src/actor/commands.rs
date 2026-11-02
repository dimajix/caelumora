//! The commands an agent carries out itself (issue 269), the same for both
//! kinds of agent - the `AgentActor` and the `ExternalAgentActor`: which it
//! has, and carrying one out in a turn of its own once its channel admitted
//! the request (issue 516). Its body's (`AgentBody`).

use super::body::AgentBody;
use caelumora_api::command::CommandError;
use caelumora_api::transport::EventSender;
use caelumora_domain::actor::{ActorEvent, Envelope};
use caelumora_domain::agent::AgentChannelState;
use caelumora_domain::command::{Command, CommandVisibility};
use caelumora_domain::turn::CommandRequest;
use tracing::{debug, info, warn};

impl AgentBody {
    /// The commands the agent carries out in its channel, sorted by name;
    /// who may see which is the channel's to decide (issue 516).
    pub(super) fn commands(&self) -> Vec<Command> {
        self.commands.commands()
    }

    /// Carries out the `CommandRequested` in `request` in a turn of its own,
    /// begun on `state` in the request's context continued in the agent's
    /// name (`Answering::to`): its owner's rights, whoever asked its
    /// initiator. Whether the issuer may ask for it was decided as the
    /// channel admitted the request (issue 516): the agent asks nothing and
    /// tells the issuer alone that it began (`CommandStarted`, issue 299);
    /// a command it does not carry out it answers `CommandFailed`.
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
    pub(super) async fn carry_out(
        &self,
        state: &mut AgentChannelState,
        request: &Envelope,
        output: &dyn EventSender,
    ) {
        let ActorEvent::CommandRequested { command, args, .. } = &request.event else { return };
        let issuer = request.context.sender_id;
        let (agent_id, name) = (self.agent_id(), self.scoped_name());
        let (answering, work) = self.work_for(request);
        let kept = CommandRequest { command: command.clone(), args: args.clone(), issuer };
        let turn_id = state.begin_command_turn(work.given_as(self.profile), kept);
        let Some(mut turn) = self.turn(state) else {
            warn!("Agent {} lost the turn it began for /{}", agent_id, command);
            return;
        };
        let mut started = false;
        let carried_out = match self.commands.entry(command) {
            Some(entry) => {
                let start = ActorEvent::CommandStarted {
                    command: command.clone(),
                    args: args.clone(),
                    to: issuer,
                    turn_id,
                };
                if let Err(e) = answering.send(start, output).await {
                    warn!("{:#}", e);
                }
                started = true;
                entry.provider().execute(command, args, &mut turn).await
            }
            None => Err(CommandError::Unknown(command.clone())),
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
                ActorEvent::CommandExecuted {
                    command: command.clone(),
                    args: args.clone(),
                    output: answer.text,
                    to,
                    turn_id: Some(turn_id),
                }
            }
            Err(e) => {
                drop(turn);
                // Never begun, as far as anything shows: nothing was said or
                // applied, so the turn leaves no end behind.
                state.turn = None;
                match &e {
                    CommandError::Failed { .. } => warn!(
                        "Agent {} {} could not carry out /{} for {}: {}",
                        agent_id, name, command, issuer, e
                    ),
                    _ => debug!(
                        "Agent {} {} does not carry out /{} for {}: {}",
                        agent_id, name, command, issuer, e
                    ),
                }
                ActorEvent::CommandFailed {
                    command: command.clone(),
                    reason: e.to_string(),
                    to: issuer,
                    turn_id: started.then_some(turn_id),
                }
            }
        };
        // What it was given while it carried the command out, before what
        // came of it.
        self.tell_recorded(state, output).await;
        if let Err(e) = answering.send(event, output).await {
            warn!("{:#}", e);
        }
    }
}
