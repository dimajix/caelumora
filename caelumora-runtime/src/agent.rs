use crate::llm::LLMClient;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::agent::{Agent, AgentState};
use caelumora_model::message::MessageEvent::{Complete, Partial};
use caelumora_model::message::{Message, MessageEvent, MessageFilter, MessageProcessor, ToolCall, ToolResult};
use caelumora_model::tool::ToolExecution;
use futures::{pin_mut, TryStreamExt};
use std::sync::Arc;
use futures::stream::BoxStream;
use tokio::sync::Mutex;
use uuid::Uuid;

struct LoopState {
    agent_id: Uuid,
    llm_client: Arc<dyn LLMClient>,
    state: Arc<Mutex<AgentState>>,
    // perceive_filter (input)
    // history_filter (storage)
    // context_filter
    // result_filter
}

impl LoopState {
    fn new(aloop: &AgentLoop) -> Self {
        Self {
            agent_id: aloop.agent.id.clone(),
            llm_client: Arc::clone(&aloop.llm_client),
            state: Arc::clone(&aloop.global_state),
        }
    }

    async fn perceive(&self, messages: Vec<Message>) -> Result<()> {
        let mut state_guard = self.state.lock().await;
        state_guard.history.extend(messages.to_vec());
        drop(state_guard);
        Ok(())
    }

    async fn plan(&self, _message: &Message) -> Result<Vec<ToolExecution>> {
        // Placeholder: in a real implementation, this would use an LLM
        // to decide which tools to call.
        Ok(Vec::new())
    }

    async fn reflect(&self) -> Result<()> {
        // Placeholder for post-processing and logging
        Ok(())
    }

    async fn messages(&self) -> Vec<Message> {
        let state_guard = self.state.lock().await;
        let context = state_guard.history.clone();
        drop(state_guard);
        context
    }

    async fn respond(&self, messages: &[Message]) {
        let mut state_guard = self.state.lock().await;
        state_guard.history.extend(messages.to_vec());
        drop(state_guard);
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult> {
        // 1. Execute tool
        // 2. Add tool result to history and context
        // 3. Return some message
        todo!()
    }
}


pub struct AgentLoop {
    agent: Agent,
    llm_client: Arc<dyn LLMClient>,
    global_state: Arc<Mutex<AgentState>>,
    message_filters: Vec<Box<dyn MessageFilter>>,
    runtime_manager: Arc<RuntimeManager>,
    max_iterations: i32,
}

#[async_trait]
impl MessageProcessor for AgentLoop {
    async fn send_messages(&mut self, messages: Vec<Message>) -> BoxStream<'static, Result<MessageEvent>> {
        // We move ownership into the stream block
        let state = LoopState::new(self);

        let max_iterations = self.max_iterations;
        let stream = stream! {
            // --- PHASE 1: PERCEIVE ---
            // Pre-process the incoming batch
            state.perceive(messages).await?;

            let mut loop_count = 0;
            while loop_count < max_iterations {
                loop_count += 1;

                // --- PHASE 2: PLAN ---
                // Call LLM with the current context
                // The LLM returns a stream of tokens or structured parts
                let llm_response_stream = state.llm_client.chat_completion(state.messages().await.as_slice()).await;
                pin_mut!(llm_response_stream);

                while let Some(llm_event) = llm_response_stream.try_next().await? {
                    match llm_event {
                        Partial(mut llm_chunk) => {
                            llm_chunk.sender_id = state.agent_id;
                            yield Ok(Partial(llm_chunk));
                        }
                        Complete(mut llm_message) => {
                            llm_message.sender_id = state.agent_id;
                            state.respond(&[llm_message.clone()]).await;
                            let tool_calls = llm_message.tool_calls.clone();

                            yield Ok(Complete(llm_message));

                            // Execute all tools
                            for c in tool_calls {
                                let r = state.execute(&c);
                                //yield r;
                            }
                        }
                        MessageEvent::EndOfTurn => {
                            yield Ok(MessageEvent::EndOfTurn);
                            break;
                        }
                    }
                }

                // Check if the LLM finished with a "Final Answer" or "Reflect" signal
                // TODO with "break"
            }

            // --- PHASE 4: REFLECT ---
            // After the loop or if we have enough context, perform a final reflection
            // TODO

            yield Ok(MessageEvent::EndOfTurn);
        };
        Box::pin(stream)
    }
}

impl AgentLoop {
    pub fn new(
        agent: Agent,
        runtime_manager: Arc<RuntimeManager>
    ) -> Self {
        todo!()
    }


    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    pub fn into_agent(self) -> Agent {
        self.agent
    }
}


