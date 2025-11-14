use std::{io::Write, num::NonZeroU32, sync::Arc};

use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
};
use serde::Deserialize;

use crate::models::user::{MessageOutcome, UserAction};
use crate::Env;

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
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(2048)) // Context size
        .with_n_threads(num_cpus::get() as i32) // Use all CPU cores
        .with_n_threads_batch(num_cpus::get() as i32);

    let ctx_result = model.new_context(backend, ctx_params);

    match ctx_result {
        Ok(mut ctx) => {
            let tool_call_history = if previous_tool_calls.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\nPrevious tool calls and results:\n{}",
                    previous_tool_calls.join("\n")
                )
            };

            let conversation_prompt = if summary.is_empty() {
                format!(
                    "<|im_start|>system\nYou are a conversational assistant that can also control smart devices. Respond with ONLY a JSON object with this exact structure:

{{
  \"updated_summary\": \"Your updated summary of the conversation context\",
  \"outcome\": {{\"Final\": {{\"response\": \"Your response to the user\"}}}} OR {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message like 'I'm doing X'\" | null, \"tool_call\": {{\"DeviceControl\": {{\"device\": \"...\", \"property\": \"...\", \"value\": \"...\"}}}} OR {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
}}

FIELD DESCRIPTIONS:
- updated_summary: A brief summary of the conversation context for your own future reference. Keep it concise but informative. Prioritize important context and recent details.
- outcome: Exactly ONE outcome variant:
  - Final: Use when you have a complete response for the user. Format: {{\"Final\": {{\"response\": \"Your response text\"}}}}
  - IntermediateToolCall: Use when you need to call a tool (like controlling a device or getting weather) before giving a final response. Format: {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message\" | null, \"tool_call\": {{\"DeviceControl\": {{\"device\": \"name\", \"property\": \"property\", \"value\": value}}}} OR {{\"GetWeather\": {{\"location\": \"city name or location\"}}}}}}}}

OUTCOME RULES:
1. Final: For general conversation, questions, greetings, or when you have all information needed and are ready to respond to the user. Use {{\"Final\": {{\"response\": \"...\"}}}}
   - Use Final when you have completed all necessary tool calls and can provide a complete response to the user.
2. IntermediateToolCall: For commands that require tool execution (device control or weather lookup). Use {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"I'm setting the AC...\" | null, \"tool_call\": {{\"DeviceControl\": {{\"device\": \"...\", \"property\": \"...\", \"value\": \"...\"}}}} OR {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
   - DeviceControl: For controlling smart devices (AC, lights, etc.)
   - GetWeather: For getting weather information. IMPORTANT: Only use GetWeather when the user provides a SPECIFIC location (city name, place name, etc.). Do NOT use GetWeather with vague terms like \"current location\", \"my location\", \"here\", or empty strings. If the user asks for weather without specifying a location, respond with Final asking them to provide a specific location first.
   - maybe_intermediate_response: Optional message to show user while tool executes (e.g., \"Setting AC to 27 degrees\" or \"Checking weather for London\"). Use null for silent execution.
   - You can chain multiple tool calls if needed - make one tool call, wait for results, then make another if necessary.

TOOL CALL RESULTS:
- When you see \"Previous tool calls and results\" above, these show tools that were already executed.
- Read the tool call results carefully - they tell you what was done and whether it succeeded.
- You can make additional tool calls if needed based on the results, or provide a Final response if you have everything you need.
- Example: If you see \"Tool call set AC temperature 27 | Result: Success\", you can either:
  - Make another IntermediateToolCall if more actions are needed
  - Provide Final: \"I've successfully set the AC temperature to 27 degrees.\" if you're done

EXAMPLES:

User: \"Hello!\"
{{\"updated_summary\":\"User greeted me\",\"outcome\":{{\"Final\":{{\"response\":\"Hello! How can I help you today?\"}}}}}}

User: \"Set AC to 27 degrees\"
{{\"updated_summary\":\"User wants AC set to 27 degrees\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":\"Setting AC to 27 degrees\",\"tool_call\":{{\"DeviceControl\":{{\"device\":\"AC\",\"property\":\"temperature\",\"value\":\"27\"}}}}}}}}}}

User: \"What's the weather like in London?\"
{{\"updated_summary\":\"User asked about weather in London\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{{\"GetWeather\":{{\"location\":\"London\"}}}}}}}}}}

User: \"What's the weather like?\"
{{\"updated_summary\":\"User asked about weather without specifying location\",\"outcome\":{{\"Final\":{{\"response\":\"I'd be happy to check the weather for you! Could you please tell me which city or location you'd like to know about?\"}}}}}}

User: \"Turn on the lights\"
{{\"updated_summary\":\"User wants lights turned on\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":null,\"tool_call\":{{\"DeviceControl\":{{\"device\":\"light\",\"property\":\"power\",\"value\":\"on\"}}}}}}}}}}
{}
Keep responses concise (a few sentences or less) unless the user asks for more detail.
Respond ONLY with valid JSON, no additional text.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
                    tool_call_history, msg
                )
            } else {
                format!(
                    "<|im_start|>system\nYou are a conversational assistant that can also control smart devices. Respond with ONLY a JSON object with this exact structure:

{{
  \"updated_summary\": \"Your updated summary of the conversation context\",
  \"outcome\": {{\"Final\": {{\"response\": \"Your response to the user\"}}}} OR {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message like 'I'm doing X'\" | null, \"tool_call\": {{\"DeviceControl\": {{\"device\": \"...\", \"property\": \"...\", \"value\": \"...\"}}}} OR {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
}}

FIELD DESCRIPTIONS:
- updated_summary: A brief summary of the conversation context for your own future reference. Keep it concise but informative. Prioritize important context and recent details.
- outcome: Exactly ONE outcome variant:
  - Final: Use when you have a complete response for the user. Format: {{\"Final\": {{\"response\": \"Your response text\"}}}}
  - IntermediateToolCall: Use when you need to call a tool (like controlling a device or getting weather) before giving a final response. Format: {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message\" | null, \"tool_call\": {{\"DeviceControl\": {{\"device\": \"name\", \"property\": \"property\", \"value\": value}}}} OR {{\"GetWeather\": {{\"location\": \"city name or location\"}}}}}}}}

OUTCOME RULES:
1. Final: For general conversation, questions, greetings, or when you have all information needed and are ready to respond to the user. Use {{\"Final\": {{\"response\": \"...\"}}}}
   - Use Final when you have completed all necessary tool calls and can provide a complete response to the user.
2. IntermediateToolCall: For commands that require tool execution (device control or weather lookup). Use {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"I'm setting the AC...\" | null, \"tool_call\": {{\"DeviceControl\": {{\"device\": \"...\", \"property\": \"...\", \"value\": \"...\"}}}} OR {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
   - DeviceControl: For controlling smart devices (AC, lights, etc.)
   - GetWeather: For getting weather information. IMPORTANT: Only use GetWeather when the user provides a SPECIFIC location (city name, place name, etc.). Do NOT use GetWeather with vague terms like \"current location\", \"my location\", \"here\", or empty strings. If the user asks for weather without specifying a location, respond with Final asking them to provide a specific location first.
   - maybe_intermediate_response: Optional message to show user while tool executes (e.g., \"Setting AC to 27 degrees\" or \"Checking weather for London\"). Use null for silent execution.
   - You can chain multiple tool calls if needed - make one tool call, wait for results, then make another if necessary.

TOOL CALL RESULTS:
- When you see \"Previous tool calls and results\" above, these show tools that were already executed.
- Read the tool call results carefully - they tell you what was done and whether it succeeded.
- You can make additional tool calls if needed based on the results, or provide a Final response if you have everything you need.
- Example: If you see \"Tool call set AC temperature 27 | Result: Success\", you can either:
  - Make another IntermediateToolCall if more actions are needed
  - Provide Final: \"I've successfully set the AC temperature to 27 degrees.\" if you're done

EXAMPLES:

User: \"Hello!\"
{{\"updated_summary\":\"User greeted me\",\"outcome\":{{\"Final\":{{\"response\":\"Hello! How can I help you today?\"}}}}}}

User: \"Set AC to 27 degrees\"
{{\"updated_summary\":\"User wants AC set to 27 degrees\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":\"Setting AC to 27 degrees\",\"tool_call\":{{\"DeviceControl\":{{\"device\":\"AC\",\"property\":\"temperature\",\"value\":\"27\"}}}}}}}}}}

User: \"What's the weather like in London?\"
{{\"updated_summary\":\"User asked about weather in London\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{{\"GetWeather\":{{\"location\":\"London\"}}}}}}}}}}

User: \"What's the weather like?\"
{{\"updated_summary\":\"User asked about weather without specifying location\",\"outcome\":{{\"Final\":{{\"response\":\"I'd be happy to check the weather for you! Could you please tell me which city or location you'd like to know about?\"}}}}}}

User: \"Turn on the lights\"
{{\"updated_summary\":\"User wants lights turned on\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":null,\"tool_call\":{{\"DeviceControl\":{{\"device\":\"light\",\"property\":\"power\",\"value\":\"on\"}}}}}}}}}}

Previous conversation summary:
{}{}
Based on the previous summary above, update it to reflect the new exchange. Keep it brief but informative. Drop old trivial details, keep important context, prioritize recent for non-important items.
Keep responses concise (a few sentences or less) unless the user asks for more detail.
Respond ONLY with valid JSON, no additional text.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
                    summary, tool_call_history, msg
                )
            };
            let tokens = model.str_to_token(&conversation_prompt, AddBos::Always)?;

            // Create a batch and add tokens (large size to handle long prompts with conversation history)
            let mut batch = LlamaBatch::new(8192, 1);

            for (i, token) in tokens.iter().enumerate() {
                let is_last = i == tokens.len() - 1;
                batch.add(*token, i as i32, &[0], is_last)?;
            }

            // Process the prompt
            ctx.decode(&mut batch)?;

            // Load grammar for structured JSON output
            let grammar = include_str!("../../grammars/response.gbnf");

            // Create sampler chain with grammar constraint for structured JSON output
            let mut sampler = LlamaSampler::chain_simple([
                LlamaSampler::temp(0.3),
                LlamaSampler::grammar(model, grammar, "root")
                    .expect("Failed to load grammar - check GBNF syntax"),
                LlamaSampler::dist(0), // Random sampling
            ]);

            // Generate tokens
            let max_tokens = 1000;
            let mut n_cur = batch.n_tokens();
            let mut response = String::new();

            for _ in 0..max_tokens {
                // Sample next token using the sampler chain
                let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);

                // Check for end of generation
                if model.is_eog_token(new_token) {
                    break;
                }

                // Convert token to string and add to response
                let output = model.token_to_str(new_token, Special::Tokenize)?;

                // Print token as it's generated
                print!("{}", output);
                std::io::stdout().flush().unwrap();

                response.push_str(&output);

                // Prepare next batch
                batch.clear();
                batch.add(new_token, n_cur, &[0], true)?;
                n_cur += 1;

                // Decode
                ctx.decode(&mut batch)?;
            }

            println!(); // Newline after streaming tokens

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
    // Get decision from LLM
    let llm_result =
        get_response_from_llm(env.llm.as_ref(), &msg, &summary, &previous_tool_calls).await;

    // Debug print the full LLM decision result
    eprintln!("[DEBUG] llm_result: {:#?}", llm_result);

    // Return the LLM decision with updated summary and outcome
    match llm_result {
        Ok(llm_response) => UserAction::LLMDecisionResult(Arc::new(Ok((
            llm_response.updated_summary,
            llm_response.outcome,
        )))),
        Err(err) => UserAction::LLMDecisionResult(Arc::new(Err(err))),
    }
}
