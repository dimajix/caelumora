use std::sync::Arc;
use caelumora_model::tool::ToolExecutor;

pub struct RuntimeManager {
    tool_executor: Arc<dyn ToolExecutor>,
}

impl RuntimeManager {
    pub fn new(tool_executor: Arc<dyn ToolExecutor>) -> Self {
        Self {
            tool_executor,
        }
    }

    pub fn tool_executor(&self) -> Arc<dyn ToolExecutor> {
        self.tool_executor.clone()
    }
}
