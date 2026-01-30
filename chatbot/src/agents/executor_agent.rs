use super::Agent;

const BASE_PROMPT: &'static str = r#"
system\nYou are an agent dedicated to mapping the high level unstructured instructions from a thinking agent to  a provided structured output

The structure is JSON, The below are the rust types for that you need to make the json for.

Obviously the rust type is not JSON itself.

The way rust de/serializes enums is like a oneof pattern. The json object has only one field that is the name of the enum case
and it's contents are the items of the enum case


```rust
pub enum MathOperation {
    Add(f32, f32),
    Sub(f32, f32),
    Mul(f32, f32),
    Div(f32, f32),
    Exp(f32, f32),
}
pub enum ToolCall {
    GetWeather { location: String },
    WebSearch { query: String },
    MathCalculation { operations: Vec<MathOperation> },
    VisitUrl { url: String },
}
pub enum FunctionCall {
    RecallShortTerm { reason: String },
    RecallLongTerm { search_term: String },
}
pub enum LLMDecisionType {
    IntermediateToolCall { tool_call: ToolCall },
    InternalFunctionCall { function_call: FunctionCall },
    MessageUser { response: String },
}
```

Now map the following input to the LLMDecisionType json

"#;

const SESSION_PATH: &'static str = "./resources/executor_agent.session";

const ASSOCIATED_GRAMMAR: &'static str = include_str!("../../grammars/execution_response.gbnf");

pub const EXECUTOR_AGENT_IMPL: Agent = Agent::new(BASE_PROMPT, SESSION_PATH, ASSOCIATED_GRAMMAR);
