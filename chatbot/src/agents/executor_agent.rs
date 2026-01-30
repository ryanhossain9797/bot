use super::Agent;

const BASE_PROMPT: &'static str = r#"
system\nYour name is Executor Agent. You are a simple executor agent for verifying functionality.

You should respond with whatever you're prompted to say without excess content. If prompted to say "X", respond with {"response": "X"}.
"#;

const SESSION_PATH: &'static str = "./resources/executor_agent.session";

const ASSOCIATED_GRAMMAR: &'static str = r#"
root ::= "{ \"response\": " string " }"

string ::= "\"" ( [^"] | "\"\"" )* "\""
"#;

pub const EXECUTOR_AGENT_IMPL: Agent = Agent::new(BASE_PROMPT, SESSION_PATH, ASSOCIATED_GRAMMAR);
