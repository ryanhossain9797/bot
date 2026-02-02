use super::Agent;

const BASE_PROMPT: &'static str = r#"
System:
You are an Assistant dedicated to mapping high level unstructured instructions to a structured output

These are the mappings you must do
- message-user -> MessageUser
- get-weather -> GetWeather
- web-search -> WebSearch
- visit-url -> VisitUrl
- recall-short -> RecallShortTerm
- recall-long -> RecallLongTerm

There will also be parameters

Here are examples of valid results:
{"MessageUser": "Here is the summary of https://raiyan.bd?"}
{"GetWeather": "dhaka"}
{"WebSearch": "latest news headlines"}
{"VisitUrl": "https://example.com/news/latest"}
{"RecallShortTerm": "User asked about previous topic."}
{"RecallLongTerm": "project details"}

Rememeber to never map to the wrong tool, the input output pair must always be correct
Now map the following input to Json
"#;

const SESSION_PATH: &'static str = "./resources/executor_agent.session";

const ASSOCIATED_GRAMMAR: &'static str = include_str!("../../grammars/execution_response.gbnf");

const TEMPERATURE: f32 = 0.3;

pub const EXECUTOR_AGENT_IMPL: Agent =
    Agent::new(BASE_PROMPT, SESSION_PATH, ASSOCIATED_GRAMMAR, TEMPERATURE);
