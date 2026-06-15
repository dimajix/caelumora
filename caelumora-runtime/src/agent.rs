use crate::message::{MessageEvent};
use crate::runtime::RuntimeManager;
use crate::security::SecurityContext;
use crate::component::{
    ComponentContainer, ContextProvider, EmitOutProcessor, MessageProcessor, PerceiveInProcessor,
    PlanPreProcessor, ComponentImpl, TurnContext,
    STATE_CONTEXT_APPENDER_KIND, STATE_CONTEXT_PROVIDER_KIND, STATE_HISTORY_WRITER_KIND,
};
use anyhow::Result;
use async_stream::stream;
use std::sync::atomic::{AtomicU8, Ordering};
use caelumora_model::agent::{Agent, AgentState, ProcessingState};
use caelumora_model::component::{
    ComponentFailPolicy, ComponentNodeConfig, ComponentConfig, STAGE_PERCEIVE
};
use caelumora_model::message::{format_conversation, ActorId, Message, MessageRole, ToolCall, ToolResult};
use caelumora_model::store::StoreError;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use std::sync::Arc;
use log::{debug, warn};
use tokio::sync::Mutex;

#[derive(Debug)]
struct AgentLoopState {
    agent: Arc<Agent>,
    runtime_manager: Arc<RuntimeManager>,
    components: ComponentContainer,
    context_provider: Arc<dyn ContextProvider>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    plan_pre_processors: Vec<ComponentImpl<dyn PlanPreProcessor>>,
}
impl AgentLoopState {
    fn new(agent: Arc<Agent>,
           runtime_manager: Arc<RuntimeManager>,
           pipeline_config: ComponentConfig,
    ) -> Result<Self> {
        let store = runtime_manager.state_store();
        let state = match store.get_agent_state(agent.id) {
            Ok(s) => s,
            Err(StoreError::NotFound{entity: _, id:_}) => {
                let state = AgentState::default();
                store.put_agent_state(agent.id, &state)?;
                state
            }
            Err(e) => {
                Err(e)?
            }
        };
        let components = ComponentContainer::from_config(
            &pipeline_config,
            runtime_manager.component_registry().as_ref(),
            Some(state),
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

        Ok(Self {
            agent,
            runtime_manager: runtime_manager.clone(),
            components,
            context_provider,
            perceive_in_processors,
            emit_out_processors,
            plan_pre_processors,
        })
    }

    async fn perceive(&mut self, messages: Vec<Message>, turn_context: &mut TurnContext) -> () {
        let original_messages = messages.clone();
        let perceived = Self::apply_stage_processors(&self.perceive_in_processors, messages, turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("PerceiveIn stage failed: {}", e);
                original_messages.clone()
            });

        let _ = perceived;
    }

    async fn respond_complete(&mut self, message: Message, turn_context: &mut TurnContext) -> Option<Message> {
        let mut output = Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
            .await
            .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }

    async fn reflect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn respond_partial(&mut self, message: Message, turn_context: &mut TurnContext) -> Option<Message> {
        let mut output = Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
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
            .current_context()
            .await
            .unwrap_or_else(|e| {
                warn!("Failed to read context from provider: {}", e);
                Vec::new()
            });
        let context = Self::apply_stage_processors(&self.plan_pre_processors, context.clone(), turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("PlanPre stage failed: {}", e);
                context
            });

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

        debug!("Sending chat request to LLM client:\n\n--START[{}]---------------------\n{}\n--END[{}]------------------------------",
            self.agent.metadata.name,
            format_conversation(context.as_slice()),
            self.agent.metadata.name);
        llm_client.chat_completion(context.as_slice()).await
    }

    async fn execute(&self, _call: &ToolCall, _security: &SecurityContext) -> Result<ToolResult> {
        // 1. Execute tool
        // 2. Add tool result to history and context
        // 3. Return some message
        todo!()
    }

    async fn apply_stage_processors<T>(
        processors: &[ComponentImpl<T>],
        mut input: Vec<Message>,
        context: &mut TurnContext,
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
    state: Arc<Mutex<AgentLoopState>>,
    agent: Arc<Agent>,
    max_iterations: i32,
    processing_state: Arc<AtomicU8>,
}

impl AgentLoop {
    pub fn new(
        agent: Agent,
        runtime_manager: Arc<RuntimeManager>
    ) -> Result<Self> {
        let personality = runtime_manager.template_store().get_personality(agent.personality_id)?;
        let agent = Arc::new(agent);
        let state = AgentLoopState::new(agent.clone(), runtime_manager, personality.components)?;
        Ok(Self {
            id: ActorId::Agent(agent.id),
            state: Arc::new(Mutex::new(state)),
            agent,
            max_iterations: 10,
            processing_state: Arc::new(AtomicU8::new(ProcessingState::Idle.into())),
        })
    }

    pub fn id(&self) -> ActorId {
        self.id
    }

    pub fn name(&self) -> String {
        self.agent.metadata.name.clone()
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
        let agent_id = ActorId::Agent(self.agent.id);
        let agent_name = self.agent.metadata.name.clone();
        let max_iterations = self.max_iterations;
        let security_context = security_context.next(self.id());
        let processing_state = self.processing_state.clone();

        let stream = stream! {
            processing_state.store(ProcessingState::Active.into(), Ordering::SeqCst);
            let mut state = state_arc.lock().await;
            let mut loop_count = 0;
            let mut turn_context = TurnContext::default();
            while loop_count < max_iterations {
                loop_count += 1;

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
                            if let Some(msg) = state.respond_complete(message, &mut turn_context).await {
                                yield Ok(MessageEvent::Complete(msg));
                            }
                            // Execute all tools
                            for c in tool_calls {
                                let _r = state.execute(&c, &security_context);
                                // Send back to llm!
                                //yield r;
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
                // TODO with "break"
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

fn default_agent_pipeline_config() -> ComponentConfig {
    ComponentConfig {
        component_nodes: vec![
            ComponentNodeConfig {
                id: STATE_HISTORY_WRITER_KIND.to_string(),
                kind: STATE_HISTORY_WRITER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: Vec::new(),
                tags: vec![STAGE_PERCEIVE.to_string()],
                fail_policy: ComponentFailPolicy::FailClosed,
            },
            ComponentNodeConfig {
                id: STATE_CONTEXT_APPENDER_KIND.to_string(),
                kind: STATE_CONTEXT_APPENDER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: vec![STATE_HISTORY_WRITER_KIND.to_string()],
                tags: vec![STAGE_PERCEIVE.to_string()],
                fail_policy: ComponentFailPolicy::FailClosed,
            },
            ComponentNodeConfig {
                id: STATE_CONTEXT_PROVIDER_KIND.to_string(),
                kind: STATE_CONTEXT_PROVIDER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: vec![STATE_CONTEXT_APPENDER_KIND.to_string()],
                tags: Vec::new(),
                fail_policy: ComponentFailPolicy::FailClosed,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::{ActorId, MessageId};
    use chrono::Utc;
    use std::collections::HashMap;
    use uuid::Uuid;
    use crate::component_impl::context::{StateContextAppender, StateContextProvider};
    use crate::component_impl::history::StateHistoryWriter;

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
        let state = Arc::new(Mutex::new(AgentState::default()));
        let processor = StateHistoryWriter {
            state: state.clone(),
        };
        let input = vec![make_message("hello")];

        let output = processor
            .process_messages(input.clone(), &mut TurnContext::default())
            .await
            .expect("history processor should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let guard = state.lock().await;
        let history = guard.history.messages();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, input[0].id);
    }

    #[tokio::test]
    async fn state_context_appender_is_passthrough_and_updates_provider_context() {
        let state = Arc::new(Mutex::new(AgentState::default()));
        let appender = StateContextAppender {
            state: state.clone(),
        };
        let provider = StateContextProvider { state, prompt_providers: vec![] };
        let input = vec![make_message("ctx")];

        let output = appender
            .process_messages(input.clone(), &mut TurnContext::default())
            .await
            .expect("context appender should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let context = provider
            .current_context()
            .await
            .expect("context provider should succeed");
        assert_eq!(context.len(), 1);
        assert_eq!(context[0].id, input[0].id);
    }
}
