use crate::llm::LLMClient;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::agent::{AgentState, Agent};
use caelumora_model::message::MessageEvent::{Complete, Partial};
use caelumora_model::message::{Message, MessageEvent, MessageProcessor, ToolCall, ToolResult};
use futures::stream::BoxStream;
use futures::TryStreamExt;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use caelumora_model::store::StoreResult;

#[derive(Debug)]
struct LoopState {
    agent_id: Uuid,
    agent: Agent,
    state: Mutex<AgentState>,
    // perceive_filter (input)
    // history_filter (storage)
    // context_filter
    // result_filter
}
impl LoopState {
    fn new(agent: Agent, state:AgentState) -> Self {
        let agent_id = agent.id.clone();
        Self {
            agent_id,
            agent,
            state: Mutex::new(state)
        }
    }

    async fn perceive(&self, messages: Vec<Message>) {
        // TODO: Apply perceive filter
        let mut state_guard = self.state.lock().await;
        // TODO: Apply history filter
        state_guard.history.extend(messages.to_vec());
        // TODO: Apply context filter
        state_guard.context.extend(messages.to_vec());
        drop(state_guard);
    }

    async fn respond_complete(&self, message: Message) -> Option<Message> {
        // Append message to context
        let mut state_guard = self.state.lock().await;
        // TODO: Apply history filter
        let context = state_guard.context.digest();
        state_guard.history.push(message.clone(), context);
        // TODO: Apply context filter
        state_guard.context.push(message.clone());
        drop(state_guard);
        // TODO: Apply emit filter
        Some(message)
    }

    async fn reflect(&self) {

    }

    async fn respond_partial(&self, message: Message) -> Option<Message> {
        // TODO: Apply emit filter
        Some(message)
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
}


#[derive(Debug, Clone)]
pub struct AgentLoop {
    runtime_manager: Arc<RuntimeManager>,
    agent_state: Arc<LoopState>,
    max_iterations: i32,
}

#[async_trait]
impl MessageProcessor for AgentLoop {
    async fn process_messages(&mut self, messages: Vec<Message>) -> BoxStream<'static, Result<MessageEvent>> {
        // We create a new LoopState instance, which will be owned by the stream
        let state = self.clone();

        let max_iterations = self.max_iterations;
        let stream = stream! {
            // --- PHASE 1: PERCEIVE ---
            // Pre-process the incoming batch
            state.perceive(messages).await;

            let mut loop_count = 0;
            while loop_count < max_iterations {
                loop_count += 1;

                // --- PHASE 2: PLAN ---
                // Call LLM with the current context
                // The LLM returns a stream of tokens or structured parts
                let mut response = state.plan().await;

                while let Some(llm_event) = response.try_next().await? {
                    match llm_event {
                        Complete(ref llm_message) => {
                            // Execute all tools
                            let tool_calls = llm_message.tool_calls.clone();
                            for c in tool_calls {
                                let r = state.execute(&c);
                                //yield r;
                            }
                            // Emit message
                            yield Ok(llm_event);
                        },
                        Partial(ref _msg) => {
                            // Emit message
                            yield Ok(llm_event);
                        },
                        MessageEvent::EndOfTurn => {
                            yield Ok(llm_event);
                            break;
                        }
                    }
                }

                // Check if the LLM finished with a "Final Answer" or "Reflect" signal
                // TODO with "break"
                break;
            }

            // --- PHASE 4: REFLECT ---
            // After the loop or if we have enough context, perform a final reflection
            state.reflect().await?;

            yield Ok(MessageEvent::EndOfTurn);
        };
        Box::pin(stream)
    }
}

impl AgentLoop {
    pub fn new(
        agent: Agent,
        runtime_manager: Arc<RuntimeManager>
    ) -> Result<Self> {
        let store = runtime_manager.state_store();
        let state = match store.get_agent_state(agent.id) {
            Ok(s) => s,
            Err(NotFound) => {
                let state = AgentState::default();
                store.put_agent_state(agent.id, &state);
                state
            }
        };
        let agent_handler = LoopState::new(agent, state);
        Ok(Self {
            runtime_manager: runtime_manager,
            agent_state: Arc::new(agent_handler),
            max_iterations: 5
        })
    }

    pub fn agent_id(&self) -> Uuid {
        self.agent_state.agent_id
    }

    async fn perceive(&self, messages: Vec<Message>) {
        self.agent_state.perceive(messages).await
    }

    async fn plan(&self) -> BoxStream<'static, Result<MessageEvent>> {
        let llm_client = self.runtime_manager.llm_client();
        let agent_state = self.agent_state.clone();
        let agent_id = agent_state.agent_id.clone();
        let context = agent_state.context().await;
        let mut messages = llm_client.chat_completion(context.as_slice()).await;

        let result = stream! {
            while let Some(event) = messages.try_next().await? {
                match event {
                    Partial(mut message) => {
                        // Fix sender ID
                        message.sender_id = agent_id;
                        if let Some(msg) = agent_state.respond_partial(message).await {
                        // Emit message
                            yield Ok(Partial(msg));
                        }
                    },
                    Complete(mut message) => {
                        // Fix sender ID
                        message.sender_id = agent_id;
                        if let Some(msg) = agent_state.respond_complete(message).await {
                        // Emit message
                            yield Ok(Complete(msg));
                        }
                    },
                    MessageEvent::EndOfTurn => {
                        yield Ok(MessageEvent::EndOfTurn);
                        break;
                    }
                }
            }
        };
        Box::pin(result)
    }

    async fn reflect(&self) -> Result<()> {
        self.agent_state.reflect().await;
        // Placeholder for post-processing and logging
        Ok(())
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult> {
        // 1. Execute tool
        // 2. Add tool result to history and context
        // 3. Return some message
        todo!()
    }
}


