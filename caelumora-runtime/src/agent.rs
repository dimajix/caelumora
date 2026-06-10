use crate::message::{MessageEvent};
use crate::runtime::RuntimeManager;
use crate::security::SecurityContext;
use crate::component::{ComponentContainer, TurnContext};
use anyhow::Result;
use async_stream::stream;
use caelumora_model::agent::{Agent, AgentState, PipelineConfig, PipelineStage};
use caelumora_model::message::{format_conversation, ActorId, Message, MessageRole, ToolCall, ToolResult};
use caelumora_model::store::StoreError;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use std::sync::Arc;
use log::{debug, warn};
use tokio::sync::Mutex;


#[derive(Debug)]
struct AgentLoopState {
    agent: Agent,
    runtime_manager: Arc<RuntimeManager>,
    state: Mutex<AgentState>,
    components: ComponentContainer,
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
        Ok(Self {
            agent,
            runtime_manager: runtime_manager.clone(),
            state: Mutex::new(state),
            components: ComponentContainer::from_config(&pipeline_config, runtime_manager.component_registry().as_ref())?,
        })
    }
    async fn context(&self) -> Vec<Message> {
        let state_guard = self.state.lock().await;
        let context = state_guard.context.clone();
        drop(state_guard);
        context.messages()
    }

    async fn history(&self) -> Vec<Message> {
        let state_guard = self.state.lock().await;
        let history = state_guard.history.clone();
        drop(state_guard);
        history.messages()
    }

    async fn perceive(&self, messages: Vec<Message>) -> () {
        let original_messages = messages.clone();
        let mut turn_context = TurnContext::default();
        let perceived = self
            .components
            .run_stage(PipelineStage::PerceiveIn, messages, &mut turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("PerceiveIn stage failed: {}", e);
                original_messages
            });

        let history_messages = self
            .components
            .run_stage(PipelineStage::HistoryWrite, perceived.clone(), &mut turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("HistoryWrite stage failed: {}", e);
                perceived.clone()
            });

        let context_messages = self
            .components
            .run_stage(PipelineStage::ContextBuild, perceived, &mut turn_context)
            .await
            .unwrap_or_else(|e| {
                warn!("ContextBuild stage failed: {}", e);
                history_messages.clone()
            });

        let mut state_guard = self.state.lock().await;
        state_guard.history.extend(history_messages);
        state_guard.context.extend(context_messages);
        drop(state_guard);
    }

    async fn respond_complete(&self, message: Message) -> Option<Message> {
        let mut turn_context = TurnContext::default();
        let mut output = self
            .components
            .run_stage(PipelineStage::EmitOut, vec![message], &mut turn_context)
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
        let mut output = self
            .components
            .run_stage(PipelineStage::EmitOut, vec![message], &mut turn_context)
            .await
            .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }
    async fn plan(&self) -> BoxStream<'static, Result<MessageEvent>> {
        let llm_client = self.runtime_manager.llm_client();
        let context = self.context().await;
        let mut turn_context = TurnContext::default();
        let context = self
            .components
            .run_stage(PipelineStage::PlanPre, context.clone(), &mut turn_context)
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
