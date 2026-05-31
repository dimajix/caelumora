use crate::message::{MessageEvent, MessageProcessor};
use crate::runtime::RuntimeManager;
use crate::security::SecurityContext;
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::agent::{Agent, AgentId, AgentState};
use caelumora_model::message::{format_conversation, ActorId, Message, MessageRole, ToolCall, ToolResult};
use caelumora_model::store::StoreError;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use std::sync::Arc;
use log::{debug, info, warn};
use tokio::sync::Mutex;


#[derive(Debug)]
pub struct AgentLoop {
    agent: Arc<Agent>,
    runtime_manager: Arc<RuntimeManager>,
    state: Arc<Mutex<AgentState>>,
    max_iterations: i32
    // perceive_filter (input)
    // history_filter (storage)
    // context_filter
    // result_filter
}

#[async_trait]
impl MessageProcessor for AgentLoop {
    fn id(&self) -> ActorId {
        ActorId::Agent(self.agent.id)
    }

    fn kind(&self) -> String {
        "agent".to_string()
    }

    fn name(&self) -> String {
        self.agent.metadata.name.clone()
    }

    async fn process_messages(&self, messages: Vec<Message>, security_context: &SecurityContext) -> BoxStream<'static, Result<MessageEvent>> {
        let state = self.clone();
        let agent_id = ActorId::Agent(state.agent.id);
        let agent_name = state.agent.metadata.name.clone();
        let max_iterations = self.max_iterations;
        let security_context = security_context.clone();

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

impl AgentLoop {
    pub fn new(
        agent: Agent,
        runtime_manager: Arc<RuntimeManager>
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
            agent: Arc::new(agent),
            runtime_manager,
            state: Arc::new(Mutex::new(state)),
            max_iterations: 10
        })
    }

    fn clone(&self) -> Self {
        Self {
            agent: self.agent.clone(),
            runtime_manager: self.runtime_manager.clone(),
            state: self.state.clone(),
            max_iterations: 0,
        }
    }

    pub fn agent_id(&self) -> AgentId {
        self.agent.id
    }

    pub async fn context(&self) -> Vec<Message> {
        let state_guard = self.state.lock().await;
        let context = state_guard.context.clone();
        drop(state_guard);
        context.messages()
    }

    pub async fn history(&self) -> Vec<Message> {
        let state_guard = self.state.lock().await;
        let history = state_guard.history.clone();
        drop(state_guard);
        history.messages()
    }

    async fn perceive(&self, messages: Vec<Message>) -> () {
        // TODO: Apply perceive filter
        let mut state_guard = self.state.lock().await;
        // TODO: Apply history filter
        state_guard.history.extend(messages.clone());
        // TODO: Apply context filter
        state_guard.context.extend(messages);
        drop(state_guard);
    }

    async fn respond_complete(&self, message: Message) -> Option<Message> {
        // Append message to context
        //let mut state_guard = self.state.lock().await;
        // TODO: Apply history filter
        //let context = state_guard.context.digest();
        // state_guard.history.push(message.clone(), context);
        // TODO: Apply context filter
        //state_guard.context.push(message.clone());
        //drop(state_guard);
        // TODO: Apply emit filter
        Some(message)
    }

    async fn reflect(&self) -> Result<()> {
        Ok(())
    }

    async fn respond_partial(&self, message: Message) -> Option<Message> {
        // TODO: Apply emit filter
        Some(message)
    }
    async fn plan(&self) -> BoxStream<'static, Result<MessageEvent>> {
        let llm_client = self.runtime_manager.llm_client();
        let agent_state = self.clone();
        let context = agent_state.context().await;

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
