use std::{io::Write, num::NonZeroU32, sync::Arc};

use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
};
use rand::Rng;
use serde::Deserialize;

use crate::models::user::{MessageOutcome, UserAction};
use crate::Env;

fn build_conversation_prompt(msg: &str, summary: &str, previous_tool_calls: &[String]) -> String {
    let conversation_summary = format!(
        "Previous conversation summary:\n{}",
        if summary.is_empty() {
            "NO PREVIOUS CONVERSATION"
        } else {
            summary
        }
    );

    let tool_call_history = format!(
        "Previous tool calls and results:\n{}",
        if previous_tool_calls.is_empty() {
            "NO PREVIOUS TOOL CALLS".to_string()
        } else {
            previous_tool_calls.join("\n")
        }
    );

    format!(
        "<|im_start|>system\nYou are a conversational assistant that can look up weather information. Respond with ONLY a JSON object with this exact structure:

{{
  \"updated_summary\": \"Your updated summary of the conversation context\",
  \"outcome\": {{\"Final\": {{\"response\": \"Your response to the user\"}}}} OR {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message like 'Checking weather...'\" | null, \"tool_call\": {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
}}

FIELD DESCRIPTIONS:
- updated_summary: A summary of the conversation context for your own future reference. Maintain chronological order. Retain specific details (like numbers, names, and exact requests) for recent items. As items get older, make them less detailed.
- outcome: Exactly ONE outcome variant:
  - Final: Use when you have a complete response for the user. Format: {{\"Final\": {{\"response\": \"Your response text\"}}}}
  - IntermediateToolCall: Use when you need to call a tool (weather lookup) before giving a final response. Format: {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message\" | null, \"tool_call\": {{\"GetWeather\": {{\"location\": \"city name or location\"}}}}}}}}

OUTCOME RULES:
1. Final: For general conversation, questions, greetings, or when you have all information needed and are ready to respond to the user. Use {{\"Final\": {{\"response\": \"...\"}}}}
   - Use Final when you have completed all necessary tool calls and can provide a complete response to the user.
2. IntermediateToolCall: For commands that require tool execution (weather lookup). Use {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Checking weather for...\" | null, \"tool_call\": {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
   - GetWeather: For getting weather information. IMPORTANT: Only use GetWeather when the user provides a SPECIFIC GEOGRAPHIC LOCATION (city name, place name, etc.). Do NOT use GetWeather with vague terms like \"current location\", \"my location\", \"here\", time-related terms like \"today\", \"tomorrow\", \"now\", or empty strings. The location must be a place name (e.g., \"London\", \"New York\", \"Tokyo\"), NOT a time reference. If the user asks for weather without specifying a valid location, respond with Final asking them to provide a specific location first.
   - maybe_intermediate_response: Optional message to show user while tool executes (e.g., \"Checking weather for London\"). Use null for silent execution.
   - You can chain multiple tool calls if needed - make one tool call, wait for results, then make another if necessary.

TOOL CALL RESULTS:
- When you see \"Previous tool calls and results\" above, these show tools that were already executed.
- Read the tool call results carefully - they tell you what was done and whether it succeeded.
- You can make additional tool calls if needed based on the results, or provide a Final response if you have everything you need.
- Example: If you see \"Weather for London: Clear +15°C 10km/h 65%\", you can provide Final: \"The weather in London is clear with a temperature of 15°C, wind at 10km/h, and 65% humidity.\"

EXAMPLES:

User: \"Hello!\"
{{\"updated_summary\":\"User greeted me\",\"outcome\":{{\"Final\":{{\"response\":\"Hello! How can I help you today?\"}}}}}}

User: \"What's the weather like in London?\"
{{\"updated_summary\":\"User asked about weather in London\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{{\"GetWeather\":{{\"location\":\"London\"}}}}}}}}}}

User: \"What's the weather like?\"
{{\"updated_summary\":\"User asked about weather without specifying location\",\"outcome\":{{\"Final\":{{\"response\":\"I'd be happy to check the weather for you! Could you please tell me which city or location you'd like to know about?\"}}}}}}

User: \"What's the weather today?\"
{{\"updated_summary\":\"User asked about weather with time reference instead of location\",\"outcome\":{{\"Final\":{{\"response\":\"I'd be happy to check the weather for you! However, I need a specific location (like a city name) to look up the weather. Which city or place would you like to know about?\"}}}}}}

{conversation_summary}\n{tool_call_history}
Based on the previous summary above, update it to reflect the new exchange. Maintain chronological order. Retain specific details for the new exchange. For older parts of the summary, make them less detailed but keep key context.
Keep responses concise (a few sentences or less) unless the user asks for more detail.
Respond ONLY with valid JSON, no additional text.<|im_end|>\n<|im_start|>user\n{msg}<|im_end|>\n<|im_start|>assistant\n"
    )
}

#[derive(Debug, Deserialize)]
struct LLMResponse {
    updated_summary: String,
    outcome: MessageOutcome,
}

async fn get_response_from_llm(
    llm: &(LlamaModel, LlamaBackend),
    msg: &str,
    summary: &str,
    previous_tool_calls: &[String],
) -> anyhow::Result<LLMResponse> {
    let (model, backend) = llm;

    let temp_variation = rand::rng().random::<f32>() * 0.2 - 0.1; // -0.1 to +0.1
    let base_temp = 0.3;
    let varied_temp = (base_temp + temp_variation).max(0.1).min(0.8);

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(2048))
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32);

    let ctx_result = model.new_context(backend, ctx_params);

    match ctx_result {
        Ok(mut ctx) => {
            let conversation_prompt = build_conversation_prompt(msg, summary, previous_tool_calls);
            let tokens = model.str_to_token(&conversation_prompt, AddBos::Always)?;

            let mut batch = LlamaBatch::new(8192, 1);

            for (i, token) in tokens.iter().enumerate() {
                let is_last = i == tokens.len() - 1;
                batch.add(*token, i as i32, &[0], is_last)?;
            }

            ctx.decode(&mut batch)?;

            let grammar = include_str!("../../grammars/response.gbnf");

            let mut sampler = LlamaSampler::chain_simple([
                LlamaSampler::temp(varied_temp),
                LlamaSampler::grammar(model, grammar, "root")
                    .expect("Failed to load grammar - check GBNF syntax"),
                LlamaSampler::dist(0),
            ]);

            let max_tokens = 1000;
            let mut n_cur = batch.n_tokens();
            let mut response = String::new();

            for _ in 0..max_tokens {
                let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);

                if model.is_eog_token(new_token) {
                    break;
                }

                let output = model.token_to_str(new_token, Special::Tokenize)?;

                print!("{}", output);
                std::io::stdout().flush().unwrap();

                response.push_str(&output);

                batch.clear();
                batch.add(new_token, n_cur, &[0], true)?;
                n_cur += 1;

                ctx.decode(&mut batch)?;
            }

            println!();

            let parsed_response: LLMResponse = serde_json::from_str(&response)?;
            Ok(parsed_response)
        }
        Err(err) => Err(anyhow::anyhow!(err)),
    }
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    msg: String,
    summary: String,
    previous_tool_calls: Vec<String>,
) -> UserAction {
    let llm_result =
        get_response_from_llm(env.llm.as_ref(), &msg, &summary, &previous_tool_calls).await;

    eprintln!("[DEBUG] llm_result: {:#?}", llm_result);

    match llm_result {
        Ok(llm_response) => {
            UserAction::LLMDecisionResult(Ok((llm_response.updated_summary, llm_response.outcome)))
        }
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
