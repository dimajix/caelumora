use crate::llm::LLMClient;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::agent::{Agent, AgentState};
use caelumora_model::message::MessageEvent::Partial;
use caelumora_model::message::{Message, MessageEvent, MessageFilter, MessageProcessor};
use caelumora_model::tool::ToolExecution;
use futures::{pin_mut, TryStreamExt};
use std::sync::Arc;
use futures::stream::BoxStream;


pub struct AgentLoop {
    agent: Agent,
    llm_client: Arc<dyn LLMClient>,
    global_state: AgentState,
    message_filters: Vec<Box<dyn MessageFilter>>,
    runtime_manager: Arc<RuntimeManager>,
    max_iterations: i32,
}


#[async_trait]
impl MessageProcessor for AgentLoop {
    async fn send_messages(&mut self, messages: &[Message]) -> BoxStream<'static, Result<MessageEvent>> {
        // We move ownership into the stream block
        let agent_id = self.agent.id.clone();
        let llm = Arc::clone(&self.llm_client);
        let mut my_messages = messages.to_vec();
        let mut global_state = self.global_state.clone();
        let mut context = global_state.private_history.clone();
        let max_iterations = self.max_iterations;

        let stream = stream! {
            // --- PHASE 1: PERCEIVE ---
            // Pre-process the incoming batch
            my_messages = self.perceive(my_messages).await?;
            global_state.private_history.extend(my_messages.clone());
            context.extend(my_messages);

            // Internal state for the ReAct loop
            let mut loop_count = 0;

            // --- THE ReAct LOOP (Plan -> Act -> Plan) ---
            while loop_count < max_iterations {
                loop_count += 1;

                // --- PHASE 2: PLAN ---
                // Call LLM with the current context
                // The LLM returns a stream of tokens or structured parts
                let llm_response_stream = llm.chat_completion(context.as_slice()).await;

                let mut current_plan_part = String::new();

                pin_mut!(llm_response_stream);

                while let Some(llm_event) = llm_response_stream.try_next().await? {
                    match llm_event {
                        MessageEvent::Partial(mut llm_chunk) => {
                            llm_chunk.sender_id = agent_id;
                            yield Ok(Partial(llm_chunk));
                        }
                        MessageEvent::Complete(mut llm_message) => {
                            llm_message.sender_id = agent_id;
                            current_plan_part.push_str(&llm_message.content.unwrap_or_default());

                            // Execute all tools
                            // TODO

                            // Add tool result to context so the next 'Plan' phase sees it
                            // TODO

                            // Yield tool result as a complete message
                            // TODO

                            // Break the inner LLM loop to re-enter the 'Plan' phase with new context
                            break;
                        }
                        MessageEvent::EndOfTurn => {
                            yield Ok(MessageEvent::EndOfTurn)
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

    async fn perceive(&self, messages: Vec<Message>) -> Result<Vec<Message>> {
        for filter in &self.message_filters {
        }

        Ok(messages)
    }

    async fn plan(&self, _message: &Message) -> Result<Vec<ToolExecution>> {
        // Placeholder: in a real implementation, this would use an LLM
        // to decide which tools to call.
        Ok(Vec::new())
    }

    async fn act(&self, executions: Vec<ToolExecution>) -> Result<Vec<Message>> {
        let _tool_executor = self.runtime_manager.tool_executor();
        for mut _execution in executions {
            // Tool execution logic would go here
        }
        Ok(Vec::new())
    }

    async fn reflect(&self) -> Result<()> {
        // Placeholder for post-processing and logging
        Ok(())
    }
}


