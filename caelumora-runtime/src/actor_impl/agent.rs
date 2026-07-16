use crate::actor::{Actor, TurnContext};
use crate::component::{ComponentContainer, ComponentImpl};
use crate::component_impl::context::ContextProvider;
use crate::component_impl::message::{EmitOutProcessor, MessageProcessor, PerceiveInProcessor};
use crate::runtime::ActorRuntime;
use crate::tool::{ToolExecutor, ToolProvider};
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, ProcessingStateHolder};
use caelumora_model::agent::{
    Agent, AgentChannelState, AgentController, AgentId, AgentState,
};
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalStatus};
use caelumora_model::channel::Channel;
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::event::GlobalEvent;
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::StateStore;
use caelumora_model::tool::{ToolCall, ToolCallState, ToolResult};
use futures::stream::BoxStream;
use log::{debug, info, warn};
use std::mem;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct AgentActor {
    id: ActorId,
    kind: String,
    agent_id: AgentId,
    agent_controller: Arc<AgentController>,
    actor_runtime: ActorRuntime,
    state_store: Arc<dyn StateStore>,

    // Components
    context_provider: Vec<ComponentImpl<dyn ContextProvider>>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    tool_executor: Box<dyn ToolExecutor>,

    // State
    state: Mutex<AgentActorState>,
    processing_state: Arc<ProcessingStateHolder>,
    max_iterations: i32,
}

#[derive(Debug)]
struct AgentActorState {
    agent_id: AgentId,
    agent_channel_state: AgentChannelState,
    agent_state: AgentState,
}

impl AgentActorState {
    fn turn_context<'a>(&'a mut self) -> TurnContext<'a> {
        TurnContext::new(
            self.agent_id,
            self.agent_channel_state.turn_event_context.sender_name.clone(),
            &mut self.agent_state,
            &mut self.agent_channel_state,
        )
    }
}

impl AgentActor {
    pub async fn new(
        channel: &Channel,
        mut agent: Agent,
        actor_runtime: ActorRuntime,
    ) -> Result<Self> {
        agent.security_context.session_id = channel.session_id;
        agent.security_context.channel_id = channel.id;
        let security_context = agent.security_context.clone();
        let personality = actor_runtime
            .entity_store()
            .get_personality(&security_context, agent.personality_id)
            .await?;
        let agent_id = agent.id;
        let agent_controller = actor_runtime
            .controller_manager()
            .new_agent_controller(channel.session_id, agent)
            .await;

        let components = ComponentContainer::from_config(
            &personality.components,
            actor_runtime.component_registry().as_ref(),
        )?;
        let context_provider = components.get_all::<dyn ContextProvider>(None)?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None)?;
        let tool_providers = components.get_all::<dyn ToolProvider>(None)?;
        let tool_executor = actor_runtime
            .new_tool_executor(tool_providers.into_iter().map(|p| p.facet).collect());

        let state_store = actor_runtime.state_store();
        let agent_channel_state = state_store
            .get_agent_channel_state(agent_id, channel.id)
            .await?;
        let agent_state = state_store.get_agent_state(agent_id).await?;
        let processing_state = agent_channel_state.processing_state.clone();
        let state = Mutex::new(AgentActorState {
            agent_id,
            agent_channel_state,
            agent_state,
        });

        Ok(AgentActor {
            id: ActorId::Agent(agent_id),
            kind: "agent".to_string(),
            agent_id,
            agent_controller,
            actor_runtime,
            state_store,
            context_provider,
            perceive_in_processors,
            emit_out_processors,
            tool_executor,
            state,
            processing_state,
            max_iterations: 10,
        })
    }

    async fn perceive<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        messages: Vec<Message>,
        event_context: &EventContext,
        security_context: &SecurityContext,
    ) {
        let agent_name = self.agent_controller.name();
        turn_context.next_turn(
            event_context.next(self.id, agent_name),
            security_context.next(self.id),
        );

        let original_messages = messages.clone();
        let perceived =
            Self::apply_stage_processors(&self.perceive_in_processors, messages, turn_context)
                .await
                .unwrap_or_else(|e| {
                    warn!("PerceiveIn stage failed: {}", e);
                    original_messages.clone()
                });

        let _ = perceived;
    }

    async fn respond_complete<'a>(
        &self,
        message: Message,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        let mut output =
            Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
                .await
                .ok()?;
        if output.is_empty() {
            return None;
        }

        let msg = output.remove(0);
        turn_context.push_message(msg.clone());

        Some(msg)
    }

    async fn reflect<'a>(&self, turn_context: &mut TurnContext<'a>) {
        turn_context.extend_state_context(turn_context.messages().clone());
        turn_context.extend_state_history(turn_context.messages().clone());
    }

    async fn respond_partial<'a>(
        &self,
        message: Message,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        let mut output =
            Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
                .await
                .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }

    async fn next_turn<'a>(&self, turn_context: &mut TurnContext<'a>) -> Result<()> {
        for context_provider in &self.context_provider {
            match context_provider.facet.current_context(turn_context).await {
                Ok(_) => {}
                Err(e) => match context_provider.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!(
                            "Component '{}' failed with fail-open policy: {}",
                            context_provider.id, e
                        );
                    }
                    ComponentFailPolicy::FailClosed => return Err(e),
                },
            }
        }

        Ok(())
    }

    async fn plan<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
    ) -> BoxStream<'static, Result<MessageEvent>> {
        let llm_client = self.actor_runtime.llm_client();

        let context = turn_context.full_context();
        let tools = self.tool_executor.tools();
        llm_client
            .chat_completion(context.as_slice(), tools.as_slice())
            .await
    }

    async fn execute_tools<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        let mut tool_calls = turn_context.take_tool_calls();
        let mut complete = true;
        for c in &mut tool_calls {
            // TODO: Check for cancellation!

            // Try to execute tool if no result is available so far...
            if c.result.is_none() {
                match c.status {
                    ApprovalStatus::Approved => {
                        match self.tool_executor.execute(&c.tool_call, turn_context).await {
                            Ok(r) => {
                                c.result = Some(r.content);
                            }
                            Err(e) => {
                                warn!("Tool execution failed: {}", e);
                                c.result = Some(format!("Error: {}", e));
                            }
                        }
                    }
                    ApprovalStatus::Denied => {
                        // TODO: Push error
                    }
                    ApprovalStatus::Timeout => {
                        // TODO: warn and push error
                    }
                    ApprovalStatus::Pending => {
                        complete = false;
                    }
                }
                // TODO: Persist results
            }
        }
        turn_context.set_tool_calls(tool_calls);

        if complete {
            let message_id = turn_context
                .last_message()
                .map(|m| m.id)
                .unwrap_or_default();

            let mut tool_results = Vec::new();
            for c in turn_context.take_tool_calls() {
                tool_results.push(ToolResult {
                    id: c.tool_call.id,
                    tool_name: c.tool_call.function_name,
                    content: c.result.unwrap(),
                })
            }
            let tool_message = Message {
                id: MessageId(uuid::Uuid::new_v4()),
                prev_id: message_id,
                sender_id: ActorId::Agent(self.agent_id),
                sender_name: self.agent_controller.name(),
                role: MessageRole::Tool,
                tool_results,
                content: None,
                timestamp: chrono::Utc::now(),
                ..Message::default()
            };
            turn_context.push_message(tool_message.clone());
            Some(tool_message)
        } else {
            None
        }
    }

    async fn apply_stage_processors<'a, T>(
        processors: &[ComponentImpl<T>],
        mut input: Vec<Message>,
        context: &mut TurnContext<'a>,
    ) -> Result<Vec<Message>>
    where
        T: MessageProcessor + ?Sized + Send + Sync,
    {
        for stage_processor in processors {
            let current = mem::take(&mut input);
            match stage_processor
                .facet
                .process_messages(current, context)
                .await
            {
                Ok(output) => {
                    input = output;
                }
                Err(e) => match stage_processor.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!(
                            "Component '{}' failed with fail-open policy: {}",
                            stage_processor.id, e
                        );
                    }
                    ComponentFailPolicy::FailClosed => return Err(e),
                },
            }
        }
        Ok(input)
    }

    async fn handle_global_event(
        &self,
        maybe_global: Option<GlobalEvent>,
        _tx: &dyn EventSender,
    ) {
        if let Some(global_event) = maybe_global {
            self.agent_controller
                .process_global_event(&global_event)
                .await;
        }
    }

    /// Handle a single incoming event in the normal (non-approval) flow.
    async fn handle_actor_event<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        event: Envelope,
        output: &dyn EventSender,
    ) -> Result<()> {
        debug!("AgentActor {} received actor event: {:?}", self.id(), event);

        match event.event {
            ActorEvent::FullResponseGenerated(messages) => {
                self.perceive(turn_context, messages, &event.context, &event.security).await;
            }
            ActorEvent::ResponseRequested => {
                self.request_response(turn_context, &event.context, &event.security, output)
                    .await
                    .context(format!(
                        "Error when trying to respond {} {}",
                        self.id,
                        self.agent_controller.name()
                    ))?;
            }
            _ => {}
        }

        Ok(())
    }

    /// Handle a single approval event while in WaitingForApproval state.
    async fn handle_approval_event<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        event: Envelope,
        output: &dyn EventSender,
    ) -> Result<()> {
        debug!("AgentActor {} processing approval event: {:?}", self.id(), event);

        let (req, status) = match &event.event {
            ActorEvent::ApprovalGranted(r) => (r, ApprovalStatus::Approved),
            ActorEvent::ApprovalDenied(r) => (r, ApprovalStatus::Denied),
            ActorEvent::ApprovalTimedOut(r) => (r, ApprovalStatus::Timeout),
            _ => unreachable!("handle_approval_event called with non-approval event"),
        };

        if !turn_context.set_tool_approval_status(req.id, status) {
            warn!("Received approval for unknown approval id {}", req.id);
        }

        self.resume_response(turn_context, output).await
    }

    async fn resume_response<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<()> {
        info!("Resuming response for agent {}", self.id);

        if let Some(msg) = self.execute_tools(turn_context).await {
            let state = self.processing_state.transition(|c| match c {
                ActorState::WaitingForApproval => Some(ActorState::Active),
                _ => None,
            });

            if state == ActorState::Active {
                turn_context.send_event(ActorEvent::CompleteMessageGenerated(msg), output).await?;
                self.generate_response(turn_context, output).await?;
            }
        }

        Ok(())
    }

    async fn request_response<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        event_context: &EventContext,
        event_security: &SecurityContext,
        output: &dyn EventSender,
    ) -> Result<()> {
        let agent_name = self.agent_controller.name();

        let activated = self.processing_state.transition(|s| match s {
            ActorState::Idle => Some(ActorState::Active),
            _ => None,
        });
        if activated != ActorState::Active {
            info!("Returning for inactive agent {} {}", self.id, agent_name);
            return Ok(());
        }

        turn_context.next_turn(
            event_context.next(self.id, agent_name.clone()),
            event_security.next(self.id),
        );

        info!("Starting generating messages for agent {} {}", self.id, agent_name);
        turn_context.send_event(ActorEvent::ResponseStarted, output).await?;

        // Initialize next turn
        self.next_turn(turn_context).await?;

        // Generate response
        self.generate_response(turn_context, output).await
    }

    async fn request_approvals<'a>(
        &self,
        tool_calls: &Vec<ToolCall>,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<()> {
        let tools = self.tool_executor.tools();
        let tool_calls: Vec<ToolCallState> = tool_calls.iter().map(|tc| {
            let tool = tools.iter().find(|t| t.name == tc.function_name);
            let approval = tool.map(|t| t.requires_approval).unwrap_or(false);
            let description = self.tool_executor.describe(tc).unwrap_or("No description".to_string());
            ToolCallState {
                id: ApprovalId::new(),
                status: if approval { ApprovalStatus::Pending } else { ApprovalStatus::Approved },
                tool_call: tc.clone(),
                description,
                result: None,
            }
        }).collect();

        if !tool_calls.is_empty() {
            let state = self.processing_state.transition(|c| match c {
                ActorState::Active => Some(ActorState::WaitingForApproval),
                _ => None,
            });

            if state == ActorState::WaitingForApproval {
                turn_context.set_tool_calls(tool_calls);
                for tc in turn_context.tool_calls() {
                    if tc.status == ApprovalStatus::Pending {
                        let request = ApprovalRequest {
                            id: tc.id,
                            session_id: turn_context.session_id,
                            channel_id: turn_context.channel_id,
                            agent_id: self.agent_id,
                            tool_call: tc.tool_call.clone(),
                            description: tc.description.clone(),
                        };
                        turn_context.send_event(ActorEvent::ApprovalRequested(request), output).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn generate_response<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<()> {
        let agent_name = self.agent_controller.name();

        let mut loop_count = 0;
        'steps: while loop_count < self.max_iterations {
            match self.processing_state.load() {
                ActorState::Silence => {
                    info!("Stopping for silenced agent {} {}", self.id, agent_name);
                    break 'steps;
                }
                ActorState::Cancelling => {
                    info!("Stopping for cancelled agent {} {}", self.id, agent_name);
                    break 'steps;
                }
                _ => {}
            }

            loop_count += 1;

            // --- PHASE 2: PLAN ---
            let mut response = self.plan(turn_context).await;
            use futures::stream::TryStreamExt;
            while let Some(event) = response
                .try_next()
                .await
                .context("Error during planning stream")?
            {
                if self.processing_state.load() == ActorState::Cancelling {
                    info!("Stopping for cancelled agent {} {}", self.id, agent_name);
                    break 'steps;
                }

                match event {
                    MessageEvent::Partial(mut message) => {
                        message.sender_name = agent_name.clone();
                        message.sender_id = self.id;
                        if let Some(msg) = self.respond_partial(message, turn_context).await {
                            turn_context.send_event(ActorEvent::PartialMessageGenerated(msg), output).await?;
                        }
                    }
                    MessageEvent::Complete(mut message) => {
                        message.sender_name = agent_name.clone();
                        message.sender_id = self.id;

                        // Check for tool approvals
                        self.request_approvals(&message.tool_calls, turn_context, output).await?;

                        if let Some(msg) = self.respond_complete(message, turn_context).await {
                            turn_context.send_event(ActorEvent::CompleteMessageGenerated(msg), output).await?;
                        }

                        // Try to execute tools
                        if turn_context.has_tool_calls() {
                            if let Some(msg) = self.execute_tools(turn_context).await {
                                turn_context.send_event(ActorEvent::CompleteMessageGenerated(msg), output).await?;
                                continue 'steps;
                            } else {
                                return Ok(());
                            }
                        }
                    }
                    MessageEvent::EndOfTurn(_) => {}
                }
            }
            break;
        }

        self.reflect(turn_context).await;

        self.processing_state.transition(|state| match state {
            ActorState::Active => Some(ActorState::Idle),
            _ => None,
        });

        // TODO: Do not send response when cancelled?
        turn_context.send_event(
            ActorEvent::FullResponseGenerated(turn_context.messages().clone()),
            output,
        ).await?;
        turn_context.finish_turn();
        info!("Finished generating messages for agent {} {}", self.id, agent_name);

        Ok(())
    }
}

fn is_approval_event(env: &Envelope) -> bool {
    matches!(
        env.event,
        ActorEvent::ApprovalGranted(_)
            | ActorEvent::ApprovalDenied(_)
            | ActorEvent::ApprovalTimedOut(_)
    )
}

#[async_trait]
impl Actor for AgentActor {
    fn id(&self) -> ActorId {
        self.id
    }
    fn kind(&self) -> String {
        self.kind.clone()
    }
    fn name(&self) -> String {
        self.agent_controller.name()
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |event| {
            matches!(
                event.event,
                ActorEvent::ApprovalGranted(_)
                    | ActorEvent::ApprovalDenied(_)
                    | ActorEvent::ApprovalTimedOut(_)
                    | ActorEvent::FullResponseGenerated(_)
                    | ActorEvent::ResponseRequested
            )
        }
    }

    async fn run(
        &self,
        mut rx: Box<dyn EventReceiver>,
        tx: Box<dyn EventSender>,
    ) -> Result<()> {
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| anyhow!("AgentActor {} already running", self.id()))?;

        info!("AgentActor {} is starting", self.id());

        let mut global_rx = self
            .actor_runtime
            .event_bus()
            .subscribe_agent(self.agent_id);

        loop {
            let current_state = self.processing_state.load();

            if current_state == ActorState::WaitingForApproval {
                tokio::select! {
                    maybe_event = rx.recv_matching(is_approval_event) => {
                        match maybe_event {
                            Some(event) => {
                                let mut turn_context = state.turn_context();
                                self.handle_approval_event(&mut turn_context, event, &*tx).await?;
                                turn_context.store_agent_state(&self.state_store).await;
                                turn_context.store_channel_state(&self.state_store).await;
                                rx.commit().await?;
                            }
                            None => break,
                        }
                    }
                    maybe_global = global_rx.recv() => {
                        self.handle_global_event(maybe_global, &*tx).await;
                    }
                }
            } else {
                tokio::select! {
                    maybe_event = rx.recv() => {
                        match maybe_event {
                            Some(event) => {
                                let mut turn_context = state.turn_context();
                                self.handle_actor_event(&mut turn_context, event, &*tx).await?;
                                turn_context.store_agent_state(&self.state_store).await;
                                turn_context.store_channel_state(&self.state_store).await;
                                rx.commit().await?;
                            }
                            None => break,
                        }
                    }
                    maybe_global = global_rx.recv() => {
                        self.handle_global_event(maybe_global, &*tx).await;
                    }
                }
            }
        }

        info!("AgentActor {} has finished", self.id());
        Ok(())
    }

    fn state(&self) -> ActorState {
        self.processing_state.load()
    }

    async fn wakeup(&self) -> Result<()> {
        info!(
            "Waking up agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        self.processing_state.store(ActorState::Idle);
        Ok(())
    }

    async fn cancel(&self) -> Result<()> {
        info!(
            "Cancelling agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        self.processing_state.store(ActorState::Cancelling);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_impl::context::{ContextProvider, StateContextAppender, StateContextProvider};
    use crate::component_impl::history::StateHistoryWriter;
    use crate::component_impl::message::MessageProcessor;
    use caelumora_model::message::MessageRole;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_message(content: &str) -> Message {
        Message {
            id: MessageId(Uuid::new_v4()),
            prev_id: MessageId::default(),
            sender_id: ActorId::None,
            sender_name: "tester".to_string(),
            role: MessageRole::User,
            content: Some(content.to_string()),
            tool_calls: Vec::new(),
            timestamp: Utc::now(),
            ..Message::default()
        }
    }

    #[tokio::test]
    async fn state_history_writer_is_passthrough_and_persists_history() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(
            AgentId::default(),
            String::new(),
            &mut agent_state,
            &mut agent_channel_state,
        );
        let processor = StateHistoryWriter {};
        let input = vec![make_message("hello")];

        let output = processor
            .process_messages(input.clone(), &mut turn_context)
            .await
            .expect("history processor should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let history = agent_channel_state.history.messages();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, input[0].id);
    }

    #[tokio::test]
    async fn state_context_appender_is_passthrough_and_updates_provider_context() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(
            AgentId::default(),
            String::new(),
            &mut agent_state,
            &mut agent_channel_state,
        );
        let appender = StateContextAppender {};
        let provider = StateContextProvider {
            prompt_providers: vec![],
        };
        let input = vec![make_message("ctx")];

        let output = appender
            .process_messages(input.clone(), &mut turn_context)
            .await
            .expect("context appender should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        provider
            .current_context(&mut turn_context)
            .await
            .expect("context provider should succeed");
        assert_eq!(turn_context.context().len(), 1);
        assert_eq!(turn_context.context()[0].id, input[0].id);
    }
}
