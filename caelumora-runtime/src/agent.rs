use crate::component::{
    ComponentContainer, ComponentImpl, TurnContext,
};
use crate::component_impl::context::ContextProvider;
use crate::component_impl::message::{EmitOutProcessor, MessageProcessor, PerceiveInProcessor, PlanPreProcessor};
use crate::message::MessageEvent;
use crate::runtime::RuntimeManager;
use crate::tool::ToolProvider;
use anyhow::Result;
use async_stream::stream;
use caelumora_model::agent::{Agent, AgentController, AgentId, AgentState, ProcessingState};
use caelumora_model::component::{
    ComponentConfig, ComponentFailPolicy
};
use caelumora_model::message::{format_conversation, ActorId, Message, MessageId, MessageRole, ToolCall, ToolResult};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::{StateStore, StoreError};
use caelumora_model::tool::ToolExecution;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use log::{debug, warn};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug)]
struct AgentLoopState {
    agent: Arc<AgentController>,
    session_id: SessionId,
    channel_id: ChannelId,
    agent_id: AgentId,
    agent_state: AgentState,
    runtime_manager: Arc<RuntimeManager>,
    state_store: Arc<dyn StateStore>,
    context_provider: Arc<dyn ContextProvider>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    plan_pre_processors: Vec<ComponentImpl<dyn PlanPreProcessor>>,
    tool_providers: Vec<ComponentImpl<dyn ToolProvider>>,
}
impl AgentLoopState {
    async fn new(session_id: SessionId,
                 channel_id: ChannelId,
            agent_controller: Arc<AgentController>,
           runtime_manager: Arc<RuntimeManager>,
           pipeline_config: ComponentConfig,
    ) -> Result<Self> {
        let store = runtime_manager.state_store();
        let agent_id = agent_controller.id();
        let agent_state = match store.get_agent_state(agent_id).await {
            Ok(s) => s,
            Err(StoreError::NotFound{entity: _, id:_}) => {
                let state = AgentState::default();
                store.put_agent_state(agent_id, &state).await?;
                state
            }
            Err(e) => {
                Err(e)?
            }
        };
        let components = ComponentContainer::from_config(
            &pipeline_config,
            runtime_manager.component_registry().as_ref()
        )?;
        let context_provider = components
            .get_all::<dyn ContextProvider>(None)
            .ok()
            .and_then(|providers| providers.into_iter().next())
            .map(|p| p.facet)
            .ok_or_else(|| anyhow::anyhow!("No context provider configured in pipeline"))?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None)?;
        let plan_pre_processors = components.get_all::<dyn PlanPreProcessor>(None)?;
        let tool_providers = components.get_all::<dyn ToolProvider>(None)?;

        Ok(Self {
            agent: agent_controller,
            session_id,
            channel_id,
            agent_id,
            agent_state,
            runtime_manager: runtime_manager.clone(),
            state_store: runtime_manager.state_store(),
            context_provider,
            perceive_in_processors,
            emit_out_processors,
            plan_pre_processors,
            tool_providers,
        })
    }

    async fn perceive(&mut self, messages: Vec<Message>, turn_context: &mut TurnContext) -> () {
        let original_messages = messages.clone();
        let perceived = Self::apply_stage_processors(&self.perceive_in_processors, messages, &mut self.agent_state, turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("PerceiveIn stage failed: {}", e);
                original_messages.clone()
            });

        let _ = perceived;

        if let Err(_e) = self.state_store.put_agent_state(self.agent_id, &self.agent_state).await {
            warn!("Cannot store state of agent {} after perceive", self.agent_id)
        }
    }

    async fn respond_complete(&mut self, message: Message, turn_context: &mut TurnContext) -> Option<Message> {
        let mut output = Self::apply_stage_processors(&self.emit_out_processors, vec![message], &mut self.agent_state, turn_context)
            .await
            .ok()?;
        if output.is_empty() {
            return None;
        }

        if let Err(_e) = self.state_store.put_agent_state(self.agent_id, &self.agent_state).await {
            warn!("Cannot store state of agent {} after respond_complete", self.agent_id)
        }
        Some(output.remove(0))
    }

    async fn reflect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn respond_partial(&mut self, message: Message, turn_context: &mut TurnContext) -> Option<Message> {
        let mut output = Self::apply_stage_processors(&self.emit_out_processors, vec![message], &mut self.agent_state, turn_context)
            .await
            .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }
    async fn plan(&mut self, turn_context: &mut TurnContext) -> BoxStream<'static, Result<MessageEvent>> {
        let llm_client = self.runtime_manager.llm_client();
        let context = self
            .context_provider
            .current_context(&self.agent_state)
            .await
            .unwrap_or_else(|e| {
                warn!("Failed to read context from provider: {}", e);
                Vec::new()
            });
        let context = Self::apply_stage_processors(&self.plan_pre_processors, context.clone(), &mut self.agent_state, turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("PlanPre stage failed: {}", e);
                context
            });

        if let Err(_e) = self.state_store.put_agent_state(self.agent_id, &self.agent_state).await {
            warn!("Cannot store state of agent {} after context preparation", self.agent_id)
        }

        if context.len() > 1 {
            let mut last_role = context.first().map(|m| m.role).unwrap();
            for m in &context[1..] {
                let next_role = m.role;
                if next_role == last_role && next_role != MessageRole::Tool {
                    let conversation = format_conversation(context.as_slice());
                    debug!("Context contains consecutive messages with same role: {}", conversation)
                }
                last_role = next_role;
            }
        }

        let agent_name = self.agent.name();
        debug!("Sending chat request to LLM client:\n\n--START[{}]---------------------\n{}\n--END[{}]------------------------------",
            agent_name,
            format_conversation(context.as_slice()),
            agent_name);

        let tools = self.tool_providers.iter().flat_map(|p| p.facet.tools()).collect::<Vec<_>>();

        llm_client.chat_completion(context.as_slice(), tools.as_slice()).await
    }

    async fn execute(&self, call: &ToolCall, _security: &SecurityContext) -> Result<ToolResult> {
        let tool_name = &call.function_name;
        for provider in &self.tool_providers {
            let tools = provider.facet.tools();
            if tools.iter().any(|t| t.name == *tool_name) {
                let mut execution = ToolExecution {
                    id: uuid::Uuid::new_v4(),
                    session_id: self.session_id,
                    agent_id: self.agent_id,
                    input: serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null),
                    output: None,
                    status: caelumora_model::tool::ExecutionStatus::Running,
                    started_at: chrono::Utc::now(),
                    finished_at: None,
                };

                let params = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                match provider.facet.execute(tool_name, params, &mut execution).await {
                    Ok(value) => {
                        execution.output = Some(value.clone());
                        execution.status = caelumora_model::tool::ExecutionStatus::Completed;
                        execution.finished_at = Some(chrono::Utc::now());
                        return Ok(ToolResult {
                            id: call.id.clone(),
                            content: value.to_string(),
                        });
                    }
                    Err(e) => {
                        execution.status = caelumora_model::tool::ExecutionStatus::Failed;
                        execution.finished_at = Some(chrono::Utc::now());
                        // TODO: Log error in execution
                        return Err(e);
                    }
                }
            }
        }
        Err(anyhow::anyhow!("Tool not found: {}", tool_name))
    }

    async fn apply_stage_processors<T>(
        processors: &[ComponentImpl<T>],
        mut input: Vec<Message>,
        state: &mut AgentState,
        context: &mut TurnContext,
    ) -> Result<Vec<Message>>
    where
        T: MessageProcessor + ?Sized + Send + Sync,
    {
        for stage_processor in processors {
            let current = std::mem::take(&mut input);
            match stage_processor.facet.process_messages(current, state, context).await {
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
    state: Arc<Mutex<AgentLoopState>>,
    agent_controller: Arc<AgentController>,
    runtime_manager: Arc<RuntimeManager>,
    max_iterations: i32,
    processing_state: Arc<AtomicU8>,
}

impl AgentLoop {
    pub async fn new(
        session_id: SessionId,
        channel_id: ChannelId,
        agent: Agent,
        runtime_manager: Arc<RuntimeManager>
    ) -> Result<Self> {
        let security_context = runtime_manager.system_security_context(session_id, channel_id);
        let personality = runtime_manager.entity_store().get_personality(&security_context, agent.personality_id).await?;
        let agent_id = agent.id;
        let event_sender = runtime_manager.event_bus().agent_event_sender(session_id, channel_id, agent_id);
        let agent_controller = Arc::new(AgentController::new(agent, event_sender, runtime_manager.entity_store()));
        let state = AgentLoopState::new(
            session_id,
            channel_id,
            agent_controller.clone(),
            runtime_manager.clone(),
            personality.components
        ).await?;
        Ok(Self {
            id: ActorId::Agent(agent_id),
            state: Arc::new(Mutex::new(state)),
            agent_controller,
            runtime_manager,
            max_iterations: 10,
            processing_state: Arc::new(AtomicU8::new(ProcessingState::Idle.into())),
        })
    }

    pub fn id(&self) -> ActorId {
        self.id
    }

    pub fn name(&self) -> String {
        self.agent_controller.name()
    }

    pub async fn process_update(&self, event: caelumora_model::agent::AgentUpdateEvent) -> Result<()> {
        self.agent_controller.process_update(&event).await;
        Ok(())
    }

    pub fn runtime_manager(&self) -> Arc<RuntimeManager> {
        self.runtime_manager.clone()
    }

    pub fn cancel(&self) {
        self.processing_state.store(ProcessingState::Cancelling.into(), Ordering::SeqCst);
    }

    pub async fn send_messages(&self, messages: Vec<Message>, _security_context: &SecurityContext) {
        let mut turn_context = TurnContext::default();
        let mut state = self.state.lock().await;
        state.perceive(messages, &mut turn_context).await
    }

    pub async fn request_response(&self, security_context: &SecurityContext) -> BoxStream<'static, Result<MessageEvent>> {
        let state_arc = self.state.clone();
        let agent_id = self.id;
        let agent_name = self.name();
        let max_iterations = self.max_iterations;
        let security_context = security_context.next(self.id());
        let processing_state = self.processing_state.clone();

        let stream = stream! {
            processing_state.store(ProcessingState::Active.into(), Ordering::SeqCst);
            let mut state = state_arc.lock().await;
            let mut loop_count = 0;
            let mut turn_context = TurnContext::default();
            while loop_count < max_iterations {
                let _ = loop_count;
                loop_count += 1;
                let _ = loop_count;

                // --- PHASE 2: PLAN ---
                // Call LLM with the current context
                // The LLM returns a stream of tokens or structured parts
                let mut response = state.plan(&mut turn_context).await;
                while let Some(event) = response.try_next().await? {
                    // Check for cancellation
                    if ProcessingState::from(processing_state.load(Ordering::SeqCst)) == ProcessingState::Cancelling {
                        yield Ok(MessageEvent::EndOfTurn(Vec::new()));
                        break;
                    }

                    match event {
                        MessageEvent::Partial(mut message) => {
                            message.sender_name = agent_name.clone();
                            message.sender_id = agent_id;
                            if let Some(msg) = state.respond_partial(message, &mut turn_context).await {
                                yield Ok(MessageEvent::Partial(msg));
                            }
                        },
                        MessageEvent::Complete(mut message) => {
                            message.sender_name = agent_name.clone();
                            message.sender_id = agent_id;
                            let tool_calls = message.tool_calls.clone();
                            let message_id = message.id;
                            if let Some(msg) = state.respond_complete(message, &mut turn_context).await {
                                yield Ok(MessageEvent::Complete(msg));
                            }
                            // Execute all tools
                            let mut tool_results = Vec::new();
                            for c in tool_calls {
                                match state.execute(&c, &security_context).await {
                                    Ok(r) => {
                                        tool_results.push(r);
                                    }
                                    Err(e) => {
                                        warn!("Tool execution failed: {}", e);
                                        tool_results.push(ToolResult {
                                            id: c.id.clone(),
                                            content: format!("Error: {}", e),
                                        });
                                    }
                                }
                            }

                            if !tool_results.is_empty() {
                                // Add tool results to the context for the next iteration
                                // Since ToolResult is not directly in Message, we might need to store it in metadata
                                // or the LLMClient should know how to handle it.
                                // Actually, let's look at Message again.
                                // It seems ToolResult is meant to be sent back.
                                // If we want the LLM to see them, they must be in the context.
                                let tool_message = Message {
                                    id: MessageId(uuid::Uuid::new_v4()),
                                    prev_id: message_id,
                                    sender_id: agent_id,
                                    sender_name: agent_name.clone(),
                                    role: MessageRole::Tool,
                                    content: Some(serde_json::to_string(&tool_results).unwrap_or_default()),
                                    timestamp: chrono::Utc::now(),
                                    ..Message::default()
                                };
                                state.perceive(vec![tool_message], &mut turn_context).await;
                                // After adding tool results, we continue the loop to call LLM again
                                continue;
                            }
                        },
                        MessageEvent::EndOfTurn(mut messages) => {
                            for m in &mut messages {
                                m.sender_name = agent_name.clone();
                                m.sender_id = agent_id;
                            }
                            yield Ok(MessageEvent::EndOfTurn(messages));
                        }
                    }
                };

                // Check for cancellation after the inner loop
                if ProcessingState::from(processing_state.load(Ordering::SeqCst)) == ProcessingState::Cancelling {
                    yield Ok(MessageEvent::EndOfTurn(Vec::new()));
                    break;
                }

                // Check if the LLM finished with a "Final Answer" or "Reflect" signal
                // If there were no tool calls, we are done
                break;
            }

            // --- PHASE 4: REFLECT ---
            // After the loop or if we have enough context, perform a final reflection
            state.reflect().await?;

            processing_state.store(ProcessingState::Idle.into(), Ordering::SeqCst);
            //yield Ok(MessageEvent::EndOfTurn(Vec::new()));
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
    use std::collections::HashMap;
    use uuid::Uuid;

    fn make_message(content: &str) -> Message {
        Message {
            id: MessageId(Uuid::new_v4()),
            prev_id: MessageId::default(),
            sender_id: ActorId::None,
            sender_name: "tester".to_string(),
            role: MessageRole::User,
            content: Some(content.to_string()),
            reasoning: None,
            tool_calls: Vec::new(),
            timestamp: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn state_history_writer_is_passthrough_and_persists_history() {
        let mut state = AgentState::default();
        let processor = StateHistoryWriter {
        };
        let input = vec![make_message("hello")];

        let output = processor
            .process_messages(input.clone(), &mut state, &mut TurnContext::default())
            .await
            .expect("history processor should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let history = state.history.messages();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, input[0].id);
    }

    #[tokio::test]
    async fn state_context_appender_is_passthrough_and_updates_provider_context() {
        let mut state = AgentState::default();
        let appender = StateContextAppender {
        };
        let provider = StateContextProvider { prompt_providers: vec![] };
        let input = vec![make_message("ctx")];

        let output = appender
            .process_messages(input.clone(), &mut state, &mut TurnContext::default())
            .await
            .expect("context appender should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let context = provider
            .current_context(&state)
            .await
            .expect("context provider should succeed");
        assert_eq!(context.len(), 1);
        assert_eq!(context[0].id, input[0].id);
    }
}
