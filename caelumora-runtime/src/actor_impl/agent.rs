use crate::actor::{Actor, ActorEvent, ActorSender, Envelope, EventContext, StandardActorSender};
use crate::component::{ComponentContainer, ComponentImpl};
use crate::component_impl::context::ContextProvider;
use crate::component_impl::message::{EmitOutProcessor, MessageProcessor, PerceiveInProcessor};
use crate::message::{MessageEvent, TurnContext};
use crate::runtime::RuntimeManager;
use crate::tool::{ToolExecutor, ToolProvider};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::agent::{
    Agent, AgentChannelState, AgentController, AgentId, AgentState, ProcessingState,
};
use caelumora_model::channel::Channel;
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::event::GlobalEvent;
use caelumora_model::message::{ActorId, Message, MessageId, MessageRole, ToolCall, ToolResult};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::StateStore;
use futures::stream::BoxStream;
use log::{debug, info, warn};
use std::mem;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct AgentActor {
    id: ActorId,
    kind: String,
    channel_id: caelumora_model::session::ChannelId,
    agent_id: AgentId,
    agent_controller: Arc<AgentController>,
    runtime_manager: Arc<RuntimeManager>,
    state_store: Arc<dyn StateStore>,

    // Components
    context_provider: Vec<ComponentImpl<dyn ContextProvider>>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    tool_executor: Box<dyn ToolExecutor>,

    // State
    state: Mutex<AgentActorState>,
    security_context: SecurityContext,
    processing_state: Arc<AtomicU8>,
    max_iterations: i32,

    run_lock: Mutex<()>,
}

#[derive(Debug)]
struct AgentActorState {
    agent_channel_state: AgentChannelState,
    agent_state: AgentState,
}

impl AgentActorState {
    fn new_turn_context<'a>(
        &'a mut self,
        agent_id: AgentId,
        agent_name: String,
        security_context: SecurityContext,
    ) -> TurnContext<'a> {
        TurnContext {
            scratch: Default::default(),
            context: vec![],
            messages: vec![],
            agent_id,
            agent_name,
            agent_state: &mut self.agent_state,
            agent_channel_state: &mut self.agent_channel_state,
            security_context,
        }
    }
}

impl AgentActor {
    pub async fn new(
        channel: &Channel,
        mut agent: Agent,
        runtime_manager: Arc<RuntimeManager>,
    ) -> Result<Self> {
        agent.security_context.session_id = channel.session_id;
        agent.security_context.channel_id = channel.id;
        let security_context = agent.security_context.clone();
        let personality = runtime_manager
            .entity_store()
            .get_personality(&security_context, agent.personality_id)
            .await?;
        let agent_id = agent.id;
        let agent_controller = runtime_manager
            .controller_manager()
            .new_agent_controller(channel.session_id, agent)
            .await;

        let components = ComponentContainer::from_config(
            &personality.components,
            runtime_manager.component_registry().as_ref(),
        )?;
        let context_provider = components.get_all::<dyn ContextProvider>(None)?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None)?;
        let tool_providers = components.get_all::<dyn ToolProvider>(None)?;
        let tool_executor = runtime_manager
            .new_tool_executor(tool_providers.into_iter().map(|p| p.facet).collect());

        let state_store = runtime_manager.state_store();
        let agent_channel_state = state_store
            .get_agent_channel_state(agent_id, channel.id)
            .await?;
        let agent_state = state_store.get_agent_state(agent_id).await?;
        let processing_state = agent_channel_state.processing_state.clone();
        let state = Mutex::new(AgentActorState {
            agent_channel_state,
            agent_state,
        });

        Ok(AgentActor {
            id: ActorId::Agent(agent_id),
            kind: "agent".to_string(),
            channel_id: channel.id,
            agent_id,
            agent_controller,
            runtime_manager,
            state_store,
            context_provider,
            perceive_in_processors,
            emit_out_processors,
            tool_executor,
            state,
            security_context,
            processing_state,
            max_iterations: 10,
            run_lock: Mutex::new(()),
        })
    }

    async fn store_agent_state<'a>(&self, turn_context: &mut TurnContext<'a>, stage: &str) {
        let agent_state = &mut turn_context.agent_state;
        if let Err(_e) = self
            .state_store
            .put_agent_state(self.agent_id, agent_state)
            .await
        {
            warn!(
                "Cannot store state of agent {} after {}", self.agent_id, stage
            )
        }
    }

    async fn store_channel_state<'a>(&self, turn_context: &mut TurnContext<'a>, stage: &str) {
        let agent_channel_state = &mut turn_context.agent_channel_state;
        if let Err(_e) = self
            .state_store
            .put_agent_channel_state(self.agent_id, self.channel_id, agent_channel_state)
            .await
        {
            warn!(
                "Cannot store channel state of agent {} after {}", self.agent_id, stage
            )
        }

    }

    async fn perceive<'a>(&self, messages: Vec<Message>, turn_context: &mut TurnContext<'a>) {
        let original_messages = messages.clone();
        let perceived =
            Self::apply_stage_processors(&self.perceive_in_processors, messages, turn_context)
                .await
                .unwrap_or_else(|e| {
                    warn!("PerceiveIn stage failed: {}", e);
                    original_messages.clone()
                });

        let _ = perceived;
        self.store_channel_state(turn_context, "perceive").await;
        self.store_agent_state(turn_context, "perceive").await;
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
        turn_context.messages.push(msg.clone());

        self.store_channel_state(turn_context, "respond_complete").await;

        Some(msg)
    }

    async fn reflect<'a>(&self, turn_context: &mut TurnContext<'a>) {
        turn_context.agent_channel_state
            .context
            .extend(turn_context.messages.clone());
        turn_context.agent_channel_state
            .history
            .extend(turn_context.messages.clone());

        self.store_channel_state(turn_context, "reflect").await;
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
        let llm_client = self.runtime_manager.llm_client();

        let mut context = turn_context.context.clone();
        context.extend_from_slice(turn_context.messages.as_slice());
        let tools = self.tool_executor.tools();
        llm_client
            .chat_completion(context.as_slice(), tools.as_slice())
            .await
    }

    async fn execute<'a>(
        &self,
        tool_calls: Vec<ToolCall>,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        let mut tool_results = Vec::new();
        for c in tool_calls {
            match self.tool_executor.execute(&c, turn_context).await {
                Ok(r) => {
                    tool_results.push(r);
                }
                Err(e) => {
                    warn!("Tool execution failed: {}", e);
                    tool_results.push(ToolResult {
                        id: c.id.clone(),
                        tool_name: c.function_name.clone(),
                        content: format!("Error: {}", e),
                    });
                }
            }
        }

        if !tool_results.is_empty() {
            let message_id = turn_context
                .messages
                .last()
                .map(|m| m.id)
                .unwrap_or_default();

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
            turn_context.messages.push(tool_message.clone());
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
            let current = std::mem::take(&mut input);
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

    async fn handle_actor_events(
        &self,
        res: usize,
        buffer: &mut Vec<Envelope>,
        output: &Sender<Envelope>,
    ) -> Result<bool> {
        if res == 0 {
            return Ok(true);
        }

        debug!("AgentActor {} received actor events: {:?}", self.id(), buffer);
        let last_security = buffer.last().unwrap().security.clone();
        let last_context = buffer.last().unwrap().context.clone();
        let mut response_requested = false;

        for event in buffer.iter_mut() {
            match &mut event.event {
                ActorEvent::FullResponseGenerated(m) => {
                    self.send_messages(m, &last_security).await;
                }
                ActorEvent::ResponseRequested => {
                    response_requested = true;
                }
                _ => {}
            }
        }
        buffer.clear();

        if response_requested {
            self.request_response(&last_context, &last_security, output)
                .await
                .context(format!(
                    "Error when trying to respond {} {}",
                    self.id,
                    self.agent_controller.name()
                ))?;
        }
        Ok(false)
    }

    async fn handle_global_event(
        &self,
        maybe_global: Option<GlobalEvent>,
        _output: &Sender<Envelope>,
    ) {
        if let Some(global_event) = maybe_global {
            self.agent_controller
                .process_global_event(&global_event)
                .await;
        }
    }

    async fn send_messages(&self, messages: &mut Vec<Message>, _event_security: &SecurityContext) {
        let mut state = self.state.lock().await;
        let mut turn_context = state.new_turn_context(
            self.agent_id,
            self.agent_controller.name(),
            self.security_context.clone(),
        );
        self.perceive(mem::take(messages), &mut turn_context).await;
    }

    async fn request_response(
        &self,
        event_context: &EventContext,
        event_security: &SecurityContext,
        output: &Sender<Envelope>,
    ) -> Result<()> {
        let agent_name = self.agent_controller.name();

        // Helper function for sending events
        let send_event = async |av:ActorEvent| -> Result<()> {
            let ev = Envelope::new(
                event_security.next(self.id),
                event_context.next(self.id, agent_name.clone()),
                av,
            );
            output
                .send(ev)
                .await
                .context(format!("Actor {} failed to send event", self.id))?;
            Ok(())
        };

        // Do not respond if in stopped or canceled state!
        let current_state = ProcessingState::from(self.processing_state.load(Ordering::SeqCst));
        if current_state == ProcessingState::Sleep || current_state == ProcessingState::Cancelling {
            info!("Returning for inactive agent {} {}",self.id, agent_name);
            return Ok(());
        }
        self.processing_state.store(ProcessingState::Active.into(), Ordering::SeqCst);
        send_event(ActorEvent::ResponseStarted).await?;
        info!("Starting generating messages for agent {} {}",self.id, agent_name);

        let mut state = self.state.lock().await;
        let mut turn_context = state.new_turn_context(
            self.agent_id,
            agent_name.clone(),
            event_security.next(self.id),
        );

        self.next_turn(&mut turn_context).await?;

        let mut loop_count = 0;
        'steps: while loop_count < self.max_iterations {
            // Check if we want to break out
            let current_state = ProcessingState::from(self.processing_state.load(Ordering::SeqCst));
            if current_state == ProcessingState::Silence {
                info!("Stopping for silenced agent {} {}",self.id, agent_name);
                break 'steps;
            }

            loop_count += 1;

            // --- PHASE 2: PLAN ---
            let mut response = self.plan(&mut turn_context).await;
            use futures::stream::TryStreamExt;
            while let Some(event) = response
                .try_next()
                .await
                .context("Error during planning stream")?
            {
                // Check for cancellation
                if ProcessingState::from(self.processing_state.load(Ordering::SeqCst))
                    == ProcessingState::Cancelling
                {
                    info!("Stopping for cancelled agent {} {}",self.id, agent_name);
                    break 'steps;
                }

                match event {
                    MessageEvent::Partial(message) => {
                        let mut message = message;
                        message.sender_name = agent_name.clone();
                        message.sender_id = self.id;
                        if let Some(msg) = self.respond_partial(message, &mut turn_context).await {
                            send_event(ActorEvent::PartialMessageGenerated(msg)).await?;
                        }
                    }
                    MessageEvent::Complete(message) => {
                        let mut message = message;
                        message.sender_name = agent_name.clone();
                        message.sender_id = self.id;
                        let tool_calls = message.tool_calls.clone();
                        if let Some(msg) = self.respond_complete(message, &mut turn_context).await {
                            send_event(ActorEvent::FullResponseGenerated(vec![msg])).await?;
                        }
                        if let Some(msg) = self.execute(tool_calls, &mut turn_context).await {
                            send_event(ActorEvent::FullResponseGenerated(vec![msg])).await?;
                            continue 'steps;
                        }
                    }
                    MessageEvent::EndOfTurn(_) => {}
                }
            }
            break;
        }

        self.reflect(&mut turn_context).await;

        let current_state = ProcessingState::from(self.processing_state.load(Ordering::SeqCst));
        if current_state != ProcessingState::Sleep && current_state != ProcessingState::Cancelling {
            self.processing_state
                .store(ProcessingState::Idle.into(), Ordering::SeqCst);
        }

        send_event(ActorEvent::FullResponseGenerated(turn_context.messages.clone())).await?;
        info!("Finished generating messages for agent {} {}",self.id, agent_name);
        Ok(())
    }
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

    fn state(&self) -> caelumora_model::agent::ProcessingState {
        ProcessingState::from(self.processing_state.load(Ordering::SeqCst))
    }

    async fn wakeup(&self) -> Result<()> {
        info!(
            "Waking up agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        self.processing_state
            .store(ProcessingState::Idle.into(), Ordering::SeqCst);
        Ok(())
    }

    async fn cancel(&self) -> Result<()> {
        info!(
            "Cancelling agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        self.processing_state
            .store(ProcessingState::Cancelling.into(), Ordering::SeqCst);
        Ok(())
    }

    fn new_sender(&self, tx: Sender<Envelope>) -> Box<dyn ActorSender> {
        Box::new(StandardActorSender {
            tx,
            filter: Box::new(|event| {
                !matches!(event.event, ActorEvent::PartialMessageGenerated(_))
            }),
        })
    }

    async fn run(&self, mut input: Receiver<Envelope>, output: Sender<Envelope>) -> Result<()> {
        let _guard = self
            .run_lock
            .try_lock()
            .map_err(|_| anyhow!("AgentActor {} already running", self.id()))?;

        info!("AgentActor {} is starting", self.id());

        let agent_id = self.id.as_agent_id().unwrap();
        let mut global_rx = self
            .runtime_manager
            .event_bus()
            .subscribe_agent(agent_id);
        let mut buffer = Vec::new();

        loop {
            tokio::select! {
                res = input.recv_many(&mut buffer, 100) => {
                    if self.handle_actor_events(res, &mut buffer, &output).await? {
                        break;
                    }
                }
                maybe_global = global_rx.recv() => {
                    self.handle_global_event(maybe_global, &output).await;
                }
            }
        }

        info!("AgentActor {} has finished", self.id());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_impl::context::{StateContextAppender, StateContextProvider};
    use crate::component_impl::history::StateHistoryWriter;
    use caelumora_model::message::{ActorId, MessageId};
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
            SecurityContext::default(),
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
            SecurityContext::default(),
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
        assert_eq!(turn_context.context.len(), 1);
        assert_eq!(turn_context.context[0].id, input[0].id);
    }
}
