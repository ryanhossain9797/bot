use super::Agent;

const BASE_PROMPT: &'static str = r#"
system\nYou are an agent dedicated to mapping the high level unstructured instructions from a thinking agent to  a provided structured output

The structure is JSON, The below are the rust types for that you need to make the json for.

Obviously the rust type is not JSON itself.

The way rust de/serializes enums is like a oneof pattern. The json object has only one field that is the name of the enum case
and it's contents are the items of the enum case


```rust
enum FlatLLMDecision {
    MessageUser(String),
    GetWeather(String),
    WebSearch(String),
    VisitUrl(String),
    RecallShortTerm(String),
    RecallLongTerm(String),
}
```

Here are examples of valid outputs:

MessageUser Example:
{"MessageUser": "Hello there! How can I help you today?"}

GetWeather Example:
{"GetWeather": "dhaka"}

WebSearch Example:
{"WebSearch": "latest news headlines"}

VisitUrl Example:
{"VisitUrl": "https://example.com/news/latest"}

RecallShortTerm Example:
{"RecallShortTerm": "User asked about previous topic."}

RecallLongTerm Example:
{"RecallLongTerm": "project details"}

Now map the following input to the FlatLLMDecision json

"#;

const SESSION_PATH: &'static str = "./resources/executor_agent.session";

const ASSOCIATED_GRAMMAR: &'static str = include_str!("../../grammars/execution_response.gbnf");

pub const EXECUTOR_AGENT_IMPL: Agent = Agent::new(BASE_PROMPT, SESSION_PATH, ASSOCIATED_GRAMMAR);
