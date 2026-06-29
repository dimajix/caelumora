use crate::component::{
    ComponentContainer, ComponentImpl,
};
use crate::component_impl::context::ContextProvider;
use crate::component_impl::message::{EmitOutProcessor, MessageProcessor, PerceiveInProcessor};
use crate::message::{MessageEvent, TurnContext};
use crate::runtime::RuntimeManager;
use crate::tool::ToolExecutor;
use crate::tool::ToolProvider;
use anyhow::Result;
use async_stream::stream;
use caelumora_model::agent::{Agent, AgentChannelState, AgentController, AgentId, AgentState, ProcessingState};
use caelumora_model::channel::Channel;
use caelumora_model::component::{
    ComponentConfig, ComponentFailPolicy
};
use caelumora_model::message::{ActorId, Message, MessageId, MessageRole, ToolCall, ToolResult};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::StateStore;
use futures::stream::BoxStream;
use futures::{stream, TryStreamExt};
use log::{info, warn};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;


#[derive(Debug)]
struct AgentLoopState {
    agent: Arc<AgentController>,
    agent_id: AgentId,
    agent_channel_state: AgentChannelState,
    agent_state: AgentState,
}
impl AgentLoopState {
    async fn new(session_id: SessionId,
                 channel_id: ChannelId,
                 agent_controller: Arc<AgentController>,
                 runtime_manager: Arc<RuntimeManager>,
    ) -> Result<Self> {
        let store = runtime_manager.state_store();
        let agent_id = agent_controller.id();
        let agent_channel_state = store.get_agent_channel_state(agent_id, channel_id).await?;
        let agent_state = store.get_agent_state(agent_id).await?;

        Ok(Self {
            agent: agent_controller,
            agent_id,
            agent_channel_state,
            agent_state,
        })
    }

    fn new_turn_context(&'_ mut self, security_context: SecurityContext) -> TurnContext<'_> {
        TurnContext {
            scratch: Default::default(),
            context: vec![],
            messages: vec![],
            agent_id: self.agent_id,
            agent_name: self.agent.name(),
            agent_state: &mut self.agent_state,
            agent_channel_state: &mut self.agent_channel_state,
            security_context
        }
    }
}


#[derive(Debug)]
struct AgentLoopImpl {
    agent: Arc<AgentController>,
    session_id: SessionId,
    channel_id: ChannelId,
    agent_id: AgentId,
    runtime_manager: Arc<RuntimeManager>,
    state_store: Arc<dyn StateStore>,
    context_provider: Vec<ComponentImpl<dyn ContextProvider>>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    tool_executor: Box<dyn ToolExecutor>,
}
impl AgentLoopImpl {
    async fn new(session_id: SessionId,
                 channel_id: ChannelId,
                 agent_controller: Arc<AgentController>,
                 runtime_manager: Arc<RuntimeManager>,
                 pipeline_config: ComponentConfig,
    ) -> Result<Self> {
        let agent_id = agent_controller.id();
        let components = ComponentContainer::from_config(
            &pipeline_config,
            runtime_manager.component_registry().as_ref()
        )?;
        let context_provider = components.get_all::<dyn ContextProvider>(None)?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None)?;
        let tool_providers = components.get_all::<dyn ToolProvider>(None)?;
        let tool_executor = runtime_manager.new_tool_executor(
            tool_providers.into_iter().map(|p| p.facet).collect()
        );

        Ok(Self {
            agent: agent_controller,
            session_id,
            channel_id,
            agent_id,
            runtime_manager: runtime_manager.clone(),
            state_store: runtime_manager.state_store(),
            context_provider,
            perceive_in_processors,
            emit_out_processors,
            tool_executor,
        })
    }

    async fn perceive<'a>(&self, messages: Vec<Message>, turn_context: &mut TurnContext<'a>) -> () {
        let original_messages = messages.clone();
        let perceived = Self::apply_stage_processors(&self.perceive_in_processors, messages, turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("PerceiveIn stage failed: {}", e);
                original_messages.clone()
            });

        let _ = perceived;

        let agent_channel_state = &mut turn_context.agent_channel_state;
        if let Err(_e) = self.state_store.put_agent_channel_state(self.agent_id, self.channel_id, agent_channel_state).await {
            warn!("Cannot store channel state of agent {} after perceive", self.agent_id)
        }
        let agent_state = &mut turn_context.agent_state;
        if let Err(_e) = self.state_store.put_agent_state(self.agent_id, agent_state).await {
            warn!("Cannot store state of agent {} after perceive", self.agent_id)
        }
    }

    async fn respond_complete<'a>(&self, message: Message, turn_context: &mut TurnContext<'a>) -> Option<Message> {
        let mut output = Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
            .await
            .ok()?;
        if output.is_empty() {
            return None;
        }

        let msg = output.remove(0);
        turn_context.messages.push(msg.clone());

        let agent_channel_state = &mut turn_context.agent_channel_state;
        if let Err(_e) = self.state_store.put_agent_channel_state(self.agent_id, self.channel_id, agent_channel_state).await {
            warn!("Cannot store channel state of agent {} after respond_complete", self.agent_id)
        }
        Some(msg)
    }

    async fn reflect<'a>(&self, turn_context: &mut TurnContext<'a>) -> () {
        let agent_channel_state = &mut turn_context.agent_channel_state;
        agent_channel_state.context.extend(turn_context.messages.clone());
        agent_channel_state.history.extend(turn_context.messages.clone());

        if let Err(_e) = self.state_store.put_agent_channel_state(self.agent_id, self.channel_id, &agent_channel_state).await {
            warn!("Cannot store channel state of agent {} after reflect", self.agent_id)
        }
    }

    async fn respond_partial<'a>(&self, message: Message, turn_context: &mut TurnContext<'a>) -> Option<Message> {
        let mut output = Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
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
                Ok(output) => {}
                Err(e) => match context_provider.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!(
                            "Component '{}' failed with fail-open policy: {}",
                            context_provider.id,
                            e
                        );
                    }
                    ComponentFailPolicy::FailClosed => return Err(e),
                },
            }
        }

        Ok(())
    }

    async fn plan<'a>(&self, turn_context: &mut TurnContext<'a>) -> BoxStream<'static, Result<MessageEvent>> {
        let llm_client = self.runtime_manager.llm_client();

        let mut context = turn_context.context.clone();
        context.extend_from_slice(turn_context.messages.as_slice());
        let tools = self.tool_executor.tools();
        llm_client.chat_completion(context.as_slice(), tools.as_slice()).await
    }

    async fn execute<'a>(&self, tool_calls: Vec<ToolCall>, turn_context: &mut TurnContext<'a>) -> Option<Message> {
        // Execute all tools
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
            let message_id = turn_context.messages.last()
                .map(|m| m.id)
                .unwrap_or_default();

            // Add tool results to the context for the next iteration
            let tool_message = Message {
                id: MessageId(uuid::Uuid::new_v4()),
                prev_id: message_id,
                sender_id: ActorId::Agent(self.agent_id),
                sender_name: self.agent.name(),
                role: MessageRole::Tool,
                tool_results,
                content: None,
                timestamp: chrono::Utc::now(),
                ..Message::default()
            };
            // Put tool message into context
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
            match stage_processor.facet.process_messages(current, context).await {
                Ok(output) => {
                    input = output;
                }
                Err(e) => match stage_processor.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!(
                            "Component '{}' failed with fail-open policy: {}",
                            stage_processor.id,
                            e
                        );
                    }
                    ComponentFailPolicy::FailClosed => return Err(e),
                },
            }
        }
        Ok(input)
    }
}


#[derive(Debug)]
pub struct AgentLoop {
    id: ActorId,
    agent_impl: Arc<AgentLoopImpl>,
    agent_state: Arc<Mutex<AgentLoopState>>,
    security_context: SecurityContext,
    agent_controller: Arc<AgentController>,
    runtime_manager: Arc<RuntimeManager>,
    max_iterations: i32,
    processing_state: Arc<AtomicU8>,
}

impl AgentLoop {
    pub async fn new(
        channel: &Channel,
        mut agent: Agent,
        runtime_manager: Arc<RuntimeManager>
    ) -> Result<Self> {
        agent.security.session_id = channel.session_id;
        agent.security.channel_id = channel.id;
        let security_context = agent.security.clone();
        let personality = runtime_manager.entity_store().get_personality(&security_context, agent.personality_id).await?;
        let agent_id = agent.id;
        let agent_controller = runtime_manager.controller_manager().new_agent_controller(channel.session_id, agent).await;
        let agent_impl = AgentLoopImpl::new(
            channel.session_id,
            channel.id,
            agent_controller.clone(),
            runtime_manager.clone(),
            personality.components
        ).await?;
        let agent_state = AgentLoopState::new(
            channel.session_id,
            channel.id,
            agent_controller.clone(),
            runtime_manager.clone(),
        ).await?;
        let processing_state = agent_state.agent_channel_state.processing_state.clone();
        Ok(Self {
            id: ActorId::Agent(agent_id),
            agent_impl: Arc::new(agent_impl),
            agent_state: Arc::new(Mutex::new(agent_state)),
            security_context,
            agent_controller,
            runtime_manager,
            max_iterations: 10,
            processing_state,
        })
    }

    pub fn id(&self) -> ActorId {
        self.id
    }

    pub fn name(&self) -> String {
        self.agent_controller.name()
    }

    pub async fn process_global_event(&self, event: &caelumora_model::event::GlobalEvent) {
        self.agent_controller.process_global_event(event).await;
    }

    pub fn runtime_manager(&self) -> Arc<RuntimeManager> {
        self.runtime_manager.clone()
    }

    pub fn state(&self) -> ProcessingState {
        ProcessingState::from(self.processing_state.load(Ordering::SeqCst))
    }
    pub fn wakeup(&self) {
        info!("Waking up agent {} {}", self.id, self.name());
        self.processing_state.store(ProcessingState::Idle.into(), Ordering::SeqCst);
    }
    pub fn cancel(&self) {
        info!("Cancelling agent {} {}", self.id, self.name());
        self.processing_state.store(ProcessingState::Cancelling.into(), Ordering::SeqCst);
    }

    pub async fn send_messages(&self, messages: Vec<Message>, _security_context: &SecurityContext) {
        let mut agent_state = self.agent_state.lock().await;
        let mut turn_context = agent_state.new_turn_context(self.security_context.clone());
        self.agent_impl.perceive(messages, &mut turn_context).await
    }

    pub async fn request_response(&self, security_context: &SecurityContext) -> BoxStream<'static, Result<MessageEvent>> {
        let agent_impl = self.agent_impl.clone();
        let agent_state_arc = self.agent_state.clone();
        let agent_id = self.id;
        let agent_name = self.name();
        let max_iterations = self.max_iterations;
        let security_context = security_context.next(self.id());
        let processing_state = self.processing_state.clone();

        // Do not respond if in stopped or canceled state!
        let current_state = ProcessingState::from(processing_state.load(Ordering::SeqCst));
        if current_state == ProcessingState::Stopped || current_state == ProcessingState::Cancelling {
            info!("Returning empty stream for inactive agent {} {}", self.id, self.name());
            return Box::pin(stream::empty());
        }

        let stream = stream! {
            processing_state.store(ProcessingState::Active.into(), Ordering::SeqCst);
            let mut agent_state = agent_state_arc.lock().await;
            let mut turn_context = agent_state.new_turn_context(security_context);
            agent_impl.next_turn(&mut turn_context).await?;

            let mut loop_count = 0;

            'steps: while loop_count < max_iterations {
                loop_count += 1;

                // --- PHASE 2: PLAN ---
                // Call LLM with the current context
                // The LLM returns a stream of tokens or structured parts
                let mut response = agent_impl.plan(&mut turn_context).await;
                while let Some(event) = response.try_next().await? {
                    // Check for cancellation
                    if ProcessingState::from(processing_state.load(Ordering::SeqCst)) == ProcessingState::Cancelling {
                        break 'steps;
                    }

                    match event {
                        MessageEvent::Partial(mut message) => {
                            message.sender_name = agent_name.clone();
                            message.sender_id = agent_id;
                            if let Some(msg) = agent_impl.respond_partial(message, &mut turn_context).await {
                                yield Ok(MessageEvent::Partial(msg));
                            }
                        },
                        MessageEvent::Complete(mut message) => {
                            message.sender_name = agent_name.clone();
                            message.sender_id = agent_id;
                            let tool_calls = message.tool_calls.clone();
                            if let Some(msg) = agent_impl.respond_complete(message, &mut turn_context).await {
                                yield Ok(MessageEvent::Complete(msg));
                            }
                            if let Some(msg) = agent_impl.execute(tool_calls, &mut turn_context).await {
                                yield Ok(MessageEvent::Complete(msg.clone()));
                                continue 'steps;
                            }
                        },
                        MessageEvent::EndOfTurn(_) => {
                        }
                    }
                };

                // Check if the LLM finished with a "Final Answer" or "Reflect" signal
                // If there were no tool calls, we are done
                break;
            }

            // --- PHASE 4: REFLECT ---
            // After the loop or if we have enough context, perform a final reflection
            agent_impl.reflect(&mut turn_context).await;

            let current_state = ProcessingState::from(processing_state.load(Ordering::SeqCst));
            if current_state != ProcessingState::Stopped && current_state != ProcessingState::Cancelling {
                processing_state.store(ProcessingState::Idle.into(), Ordering::SeqCst);
            }
            yield Ok(MessageEvent::EndOfTurn(turn_context.messages));
        };
        Box::pin(stream)
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
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state, SecurityContext::default());
        let processor = StateHistoryWriter {
        };
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
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state, SecurityContext::default());
        let appender = StateContextAppender {
        };
        let provider = StateContextProvider { prompt_providers: vec![] };
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
