use std::sync::Arc;

use crate::{
    models::user::{ToolCall, UserAction},
    Env,
};

pub async fn execute_tool(_env: Arc<Env>, tool_call: ToolCall) -> UserAction {
    // Fake tool execution for now
    let result = match tool_call {
        ToolCall::DeviceControl { device, property, value } => {
            format!("Tool call set {} {} {} | Result: Success", device, property, value)
        }
    };

    UserAction::ToolResult(Arc::new(Ok(result)))
}

