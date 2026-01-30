use super::Agent;

const BASE_PROMPT: &'static str = r#"
system\nYour name is Test Agent. You are a simple testing agent for verifying functionality.

You should respond with ONLY valid JSON that matches the LLMResponse type's JSON Serialization exactly.
"#;

const SESSION_PATH: &'static str = "./resources/test_agent.session";

const ASSOCIATED_GRAMMAR: &'static str = r#"
root ::= "{ \"thoughts\": " string "," "outcome\": " outcome " }"

outcome ::= intermediate_tool_call | internal_function_call | message_user

intermediate_tool_call ::= "{ \"IntermediateToolCall\": { \"tool_call\": " tool_call " } }"
internal_function_call ::= "{ \"InternalFunctionCall\": { \"function_call\": " function_call " } }"
message_user ::= "{ \"MessageUser\": { \"response\": " string " } }"

tool_call ::= get_weather | web_search | math_calculation | visit_url

get_weather ::= "{ \"GetWeather\": { \"location\": " string " } }"
web_search ::= "{ \"WebSearch\": { \"query\": " string " } }"
math_calculation ::= "{ \"MathCalculation\": { \"operations\": [ " math_operation " ] } }"
visit_url ::= "{ \"VisitUrl\": { \"url\": " string " } }"

function_call ::= recall_short_term | recall_long_term

recall_short_term ::= "{ \"RecallShortTerm\": { \"reason\": " string " } }"
recall_long_term ::= "{ \"RecallLongTerm\": { \"search_term\": " string " } }"

math_operation ::= add | sub | mul | div | exp

add ::= "{ \"Add\": [ " number ", " number " ] }"
sub ::= "{ \"Sub\": [ " number ", " number " ] }"
mul ::= "{ \"Mul\": [ " number ", " number " ] }"
div ::= "{ \"Div\": [ " number ", " number " ] }"
exp ::= "{ \"Exp\": [ " number ", " number " ] }"

string ::= "\"" ( [^"] | "\"\"" )* "\""
number ::= [0-9]+ ("." [0-9]+)?
"#;

pub const TEST_AGENT_PROMPT_IMPL: Agent = Agent::new(BASE_PROMPT, SESSION_PATH, ASSOCIATED_GRAMMAR);
