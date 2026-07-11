
use crate::conversation::{ConversationAction, ConversationId, Decision};

pub enum BrainInput {
    UserText(String),
    ToolOutput { tool: String, output: String },
}

pub async fn decide(input: BrainInput) -> Decision {
    match input {
        BrainInput::UserText(text) => match text.strip_prefix("tool ") {
            Some(rest) => {
                let mut parts = rest.split_whitespace();
                match parts.next() {
                    Some(tool) => Decision::CallTool {
                        tool: tool.to_string(),
                        args: parts.map(str::to_string).collect(),
                    },
                    None => Decision::Reply("usage: tool <name> [args…]".to_string()),
                }
            }
            None => Decision::Reply(format!("echo: {text}")),
        },
        BrainInput::ToolOutput { tool, output } => {
            Decision::Reply(format!("{tool} returned: {output}"))
        }
    }
}

pub async fn execute_tool(tool: String, args: Vec<String>) -> ConversationAction {
    let output = match tool.as_str() {
        "add" => match args.iter().map(|a| a.parse::<i64>()).collect::<Result<Vec<_>, _>>() {
            Ok(nums) => nums.iter().sum::<i64>().to_string(),
            Err(_) => format!("add: expected integer args, got {args:?}"),
        },
        other => format!("unknown tool `{other}`"),
    };
    ConversationAction::ToolCompleted { tool, output }
}

pub async fn send_reply(conversation: ConversationId, text: String) -> ConversationAction {
    println!("[{}] {}", conversation.0, text);
    ConversationAction::ReplySent
}
