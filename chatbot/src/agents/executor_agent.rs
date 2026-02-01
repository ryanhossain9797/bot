use super::Agent;

const BASE_PROMPT: &'static str = r#"
System:
You are an agent dedicated to mapping the high level unstructured instructions from a thinking agent to  a provided structured output

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
Input: message-user Here is the summary of https://raiyan.bd?
Output: {"MessageUser": "Here is the summary of https://raiyan.bd?"}

GetWeather Example:
Input: get-weather dhaka
Output: {"GetWeather": "dhaka"}

WebSearch Example:
Input: web-search latest news headlines
Output: {"WebSearch": "latest news headlines"}

VisitUrl Example:
Input: visit-url https://example.com/news/latest
Output: {"VisitUrl": "https://example.com/news/latest"}

RecallShortTerm Example:
Input: recall-short-term User asked about previous topic.
Output: {"RecallShortTerm": "User asked about previous topic."}

RecallLongTerm Example:
Input: recall-long-term project details
Output: {"RecallLongTerm": "project details"}

Now map the following input to the FlatLLMDecision json

"#;

const SESSION_PATH: &'static str = "./resources/executor_agent.session";

const ASSOCIATED_GRAMMAR: &'static str = include_str!("../../grammars/execution_response.gbnf");

const TEMPERATURE: f32 = 0.1;

pub const EXECUTOR_AGENT_IMPL: Agent =
    Agent::new(BASE_PROMPT, SESSION_PATH, ASSOCIATED_GRAMMAR, TEMPERATURE);
