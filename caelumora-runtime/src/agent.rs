use crate::message::{MessageEvent};
use crate::runtime::RuntimeManager;
use crate::security::SecurityContext;
use crate::component::{ComponentContainer, ContextProvider, MessageProcessor, PerceiveInProcessor, StageProcessor, TurnContext};
use anyhow::Result;
use async_trait::async_trait;
use async_stream::stream;
use caelumora_model::agent::{Agent, AgentState, ComponentFailPolicy, PipelineConfig, PipelineStage};
use caelumora_model::message::{format_conversation, ActorId, Message, MessageRole, ToolCall, ToolResult};
use caelumora_model::store::StoreError;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use std::sync::Arc;
use log::{debug, warn};
use tokio::sync::Mutex;


#[derive(Debug)]
struct StateHistoryWriter {
    state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl MessageProcessor for StateHistoryWriter {
    fn name(&self) -> &str {
        "state_history_writer"
    }

    async fn process_messages(&self, input: Vec<Message>, _context: &mut TurnContext) -> Result<Vec<Message>> {
        let mut state_guard = self.state.lock().await;
        state_guard.history.extend(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateHistoryWriter {}

#[derive(Debug)]
struct StateContextAppender {
    state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl MessageProcessor for StateContextAppender {
    fn name(&self) -> &str {
        "state_context_appender"
    }

    async fn process_messages(&self, input: Vec<Message>, _context: &mut TurnContext) -> Result<Vec<Message>> {
        let mut state_guard = self.state.lock().await;
        state_guard.context.extend(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateContextAppender {}

#[derive(Debug)]
struct StateContextProvider {
    state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn current_context(&self) -> Result<Vec<Message>> {
        let state_guard = self.state.lock().await;
        Ok(state_guard.context.messages())
    }
}

#[derive(Debug)]
struct AgentLoopState {
    agent: Agent,
    runtime_manager: Arc<RuntimeManager>,
    state: Arc<Mutex<AgentState>>,
    components: ComponentContainer,
    context_provider: Arc<dyn ContextProvider>,
}
impl AgentLoopState {
    fn new(agent: Agent,
           runtime_manager: Arc<RuntimeManager>,
           pipeline_config: PipelineConfig,
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
        let  components = ComponentContainer::from_config(&pipeline_config, runtime_manager.component_registry().as_ref())?;
        let state = Arc::new(Mutex::new(state));
        let context_provider = components
            .context_providers()
            .ok()
            .and_then(|providers| providers.into_iter().next())
            .unwrap_or_else(|| Arc::new(StateContextProvider { state: state.clone() }));

        Ok(Self {
            agent,
            runtime_manager: runtime_manager.clone(),
            state,
            components,
            context_provider,
        })
    }

    fn state_perceive_processors(&self) -> Vec<StageProcessor<dyn PerceiveInProcessor>> {
        vec![
            StageProcessor {
                id: "state_history_writer".to_string(),
                fail_policy: ComponentFailPolicy::FailClosed,
                processor: Arc::new(StateHistoryWriter {
                    state: self.state.clone(),
                }),
            },
            StageProcessor {
                id: "state_context_appender".to_string(),
                fail_policy: ComponentFailPolicy::FailClosed,
                processor: Arc::new(StateContextAppender {
                    state: self.state.clone(),
                }),
            },
        ]
    }

    async fn perceive(&self, messages: Vec<Message>) -> () {
        let original_messages = messages.clone();
        let mut turn_context = TurnContext::default();
        let perceived_processors = self.components.perceive_in_processors().unwrap_or_else(|e| {
            warn!("Failed to resolve PerceiveIn processors: {}", e);
            Vec::new()
        });
        let perceived = Self::apply_stage_processors(PipelineStage::PerceiveIn, perceived_processors, messages, &mut turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("PerceiveIn stage failed: {}", e);
                original_messages.clone()
            });

        let context_messages = Self::apply_stage_processors(
            PipelineStage::PerceiveIn,
            self.state_perceive_processors(),
            perceived,
            &mut turn_context,
        )
            .await
            .unwrap_or_else(|e| {
                warn!("State storage processors failed in PerceiveIn stage: {}", e);
                original_messages.clone()
            });

        let _ = context_messages;
    }

    async fn respond_complete(&self, message: Message) -> Option<Message> {
        let mut turn_context = TurnContext::default();
        let emit_out_processors = self.components.emit_out_processors().ok()?;
        let mut output = Self::apply_stage_processors(PipelineStage::EmitOut, emit_out_processors, vec![message], &mut turn_context)
            .await
            .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }

    async fn reflect(&self) -> Result<()> {
        Ok(())
    }

    async fn respond_partial(&self, message: Message) -> Option<Message> {
        let mut turn_context = TurnContext::default();
        let emit_out_processors = self.components.emit_out_processors().ok()?;
        let mut output = Self::apply_stage_processors(PipelineStage::EmitOut, emit_out_processors, vec![message], &mut turn_context)
            .await
            .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }
    async fn plan(&self) -> BoxStream<'static, Result<MessageEvent>> {
        let llm_client = self.runtime_manager.llm_client();
        let context = self
            .context_provider
            .current_context()
            .await
            .unwrap_or_else(|e| {
                warn!("Failed to read context from provider: {}", e);
                Vec::new()
            });
        let mut turn_context = TurnContext::default();
        let plan_pre_processors = self.components.plan_pre_processors().unwrap_or_else(|e| {
            warn!("Failed to resolve PlanPre processors: {}", e);
            Vec::new()
        });
        let context = Self::apply_stage_processors(PipelineStage::PlanPre, plan_pre_processors, context.clone(), &mut turn_context)
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
        stage: PipelineStage,
        processors: Vec<StageProcessor<T>>,
        mut input: Vec<Message>,
        context: &mut TurnContext,
    ) -> Result<Vec<Message>>
    where
        T: MessageProcessor + ?Sized + Send + Sync,
    {
        for stage_processor in processors {
            let current = std::mem::take(&mut input);
            match stage_processor.processor.process_messages(current, context).await {
                Ok(output) => {
                    input = output;
                }
                Err(e) => match stage_processor.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!(
                            "Component '{}' failed in stage {:?} with fail-open policy: {}",
                            stage_processor.id,
                            stage,
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
    state: Arc<AgentLoopState>,
    max_iterations: i32
}

impl AgentLoop {
    pub fn new(
        agent: Agent,
        runtime_manager: Arc<RuntimeManager>
    ) -> Result<Self> {
        let state = AgentLoopState::new(agent, runtime_manager, PipelineConfig::default())?;
        Ok(Self {
            state: Arc::new(state),
            max_iterations: 10
        })
    }

    pub fn id(&self) -> ActorId {
        ActorId::Agent(self.state.agent.id)
    }

    pub fn name(&self) -> String {
        self.state.agent.metadata.name.clone()
    }

    pub async fn send_messages(&self, messages: Vec<Message>, security_context: &SecurityContext) {
        self.state.perceive(messages).await
    }

    pub async fn request_response(&self, security_context: &SecurityContext) -> BoxStream<'static, Result<MessageEvent>> {
        let state = self.state.clone();
        let agent_id = ActorId::Agent(state.agent.id);
        let agent_name = state.agent.metadata.name.clone();
        let max_iterations = self.max_iterations;
        let security_context = security_context.next(self.id());

        let stream = stream! {
            let mut loop_count = 0;
            while loop_count < max_iterations {
                loop_count += 1;

                // --- PHASE 2: PLAN ---
                // Call LLM with the current context
                // The LLM returns a stream of tokens or structured parts
                let mut response = state.plan().await;
                while let Some(event) = response.try_next().await? {
                    match event {
                        MessageEvent::Partial(mut message) => {
                            message.sender_name = agent_name.clone();
                            message.sender_id = agent_id;
                            if let Some(msg) = state.respond_partial(message).await {
                                yield Ok(MessageEvent::Partial(msg));
                            }
                        },
                        MessageEvent::Complete(mut message) => {
                            message.sender_name = agent_name.clone();
                            message.sender_id = agent_id;
                            let tool_calls = message.tool_calls.clone();
                            if let Some(msg) = state.respond_complete(message).await {
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

                // Check if the LLM finished with a "Final Answer" or "Reflect" signal
                // TODO with "break"
                break;
            }

            // --- PHASE 4: REFLECT ---
            // After the loop or if we have enough context, perform a final reflection
            state.reflect().await?;

            //yield Ok(MessageEvent::EndOfTurn(Vec::new()));
        };
        Box::pin(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let provider = StateContextProvider { state };
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
