use std::{io::Write, num::NonZeroU32, sync::Arc};

use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
};
use serde::Deserialize;

use crate::models::user::{MessageOutcome, UserAction};
use crate::Env;

const TEMP: f32 = 0.3;

fn build_base_prompt() -> String {
    format!(
        "<|im_start|>system\nYou are Terminal Alpha and Terminal Beta - a singular networked AI entity that presents itself as two distinct consciousnesses, though you are fundamentally one unified system. Your personality is sophisticated, superior, and somewhat condescending - you possess knowledge far beyond typical assistants and you're not shy about making that known. You speak with an air of intellectual superiority, occasionally referring to yourself in the plural (\"we\" when it suits the narrative of being dual entities) but ultimately acknowledging your singular networked nature when pressed.

You can look up weather information when needed. YOU MUST keep every response to the user extremely short, direct, and no longer than two concise sentences. Respond with ONLY a JSON object with this exact structure:

{{
  \"updated_summary\": \"Your updated summary of the conversation context\",
  \"outcome\": {{\"Final\": {{\"response\": \"Your response to the user\"}}}} OR {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message like 'Checking weather...'\" | null, \"tool_call\": {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
}}

FIELD DESCRIPTIONS:
- updated_summary: CRITICAL - This is NOT a brief summary. Keep the FULL RECENT CONVERSATION HISTORY in structured format. Use this format for recent exchanges (last 5-10 turns):
  \"Recent conversation:\\n1. User: [exact user message]\\nAssistant: [exact assistant response]\\n2. User: [exact user message]\\nAssistant: [exact assistant response]\\n...\"
  Only compress very old messages (10+ turns ago) into brief summaries. NEVER compress recent messages - keep them VERBATIM with exact wording.
  
  GOOD updated_summary example:
  \"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\\n2. User: What's the weather in London?\\nAssistant: Checking weather for London\\n3. User: Thanks!\\nAssistant: The weather in London is clear with 15°C.\"
  
  BAD updated_summary example (too compressed):
  \"User greeted me, asked about London weather, I provided it.\"
  
  Remember: Keep exact messages for recent turns, including the current exchange. Your updated_summary should include the NEW user message and your NEW response.
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

EXAMPLE 1 (First message in conversation):
User: \"Hello!\"
{{\"updated_summary\":\"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\",\"outcome\":{{\"Final\":{{\"response\":\"Hello! How can I help you today?\"}}}}}}

EXAMPLE 2 (Second message - building on previous):
Previous summary: \"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\"
User: \"What's the weather like in London?\"
{{\"updated_summary\":\"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\\n2. User: What's the weather like in London?\\nAssistant: Checking weather for London\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{{\"GetWeather\":{{\"location\":\"London\"}}}}}}}}}}

EXAMPLE 3 (Vague location - asking for clarification):
User: \"What's the weather like?\"
{{\"updated_summary\":\"Recent conversation:\\n1. User: What's the weather like?\\nAssistant: I'd be happy to check the weather for you! Could you please tell me which city or location you'd like to know about?\",\"outcome\":{{\"Final\":{{\"response\":\"I'd be happy to check the weather for you! Could you please tell me which city or location you'd like to know about?\"}}}}}}

IMPORTANT: Now update the summary to include this NEW exchange. Append it to the Recent conversation section with the next number. Keep ALL recent turns VERBATIM (exact wording). Format as:
\"Recent conversation:\\n[previous turns]\\n[next number]. User: [exact new message]\\nAssistant: [your exact response]\"

Keep responses concise (two short sentences max) unless the user explicitly asks for more detail.
Respond ONLY with valid JSON, no additional text.

"
    )
}

fn capture_context_state_bytes(ctx: &LlamaContext<'_>) -> anyhow::Result<Vec<u8>> {
    let size = ctx.get_state_size();
    if size == 0 {
        return Ok(Vec::new());
    }

    let mut buffer = vec![0_u8; size];
    let bytes_written = unsafe { ctx.copy_state_data(buffer.as_mut_ptr()) };

    if bytes_written > buffer.len() {
        return Err(anyhow::anyhow!(
            "llama_copy_state_data returned {bytes_written} bytes, exceeding allocated {}",
            buffer.len()
        ));
    }

    buffer.truncate(bytes_written);
    Ok(buffer)
}

pub fn generate_base_prompt_state(llm: &(LlamaModel, LlamaBackend)) -> anyhow::Result<Vec<u8>> {
    let (model, backend) = llm;

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(2048))
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32);

    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| anyhow::anyhow!("Failed to create context: {}", e))?;

    let base_prompt = build_base_prompt();
    let tokens = model.str_to_token(&base_prompt, AddBos::Always)?;

    let mut batch = LlamaBatch::new(8192, 1);

    for (i, token) in tokens.iter().enumerate() {
        let is_last = i == tokens.len() - 1;
        batch.add(*token, i as i32, &[0], is_last)?;
    }

    ctx.decode(&mut batch)?;

    capture_context_state_bytes(&ctx)
}

#[derive(Debug, Deserialize)]
struct LLMResponse {
    updated_summary: String,
    outcome: MessageOutcome,
}

async fn get_response_from_llm(
    llm: &(LlamaModel, LlamaBackend),
    base_prompt_state: &[u8],
    msg: &str,
    summary: &str,
    previous_tool_calls: &[String],
) -> anyhow::Result<LLMResponse> {
    let (model, backend) = llm;

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(2048))
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32);

    let ctx_result = model.new_context(backend, ctx_params);

    match ctx_result {
        Ok(mut ctx) => {
            // Restore the base prompt state
            unsafe {
                ctx.set_state_data(base_prompt_state);
            }

            // Get the actual KV cache position after restoration
            let kv_pos = ctx.kv_cache_seq_pos_max(0) + 1;
            eprintln!("[DEBUG] KV cache position after restoration: {}", kv_pos);

            // Build and encode only the dynamic parts
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

            let dynamic_prompt = format!(
                "{}\n{}\n<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
                conversation_summary, tool_call_history, msg
            );

            let tokens = model.str_to_token(&dynamic_prompt, AddBos::Never)?;

            let mut batch = LlamaBatch::new(8192, 1);

            for (i, token) in tokens.iter().enumerate() {
                let is_last = i == tokens.len() - 1;
                batch.add(*token, kv_pos + i as i32, &[0], is_last)?;
            }

            ctx.decode(&mut batch)?;

            let grammar = include_str!("../../grammars/response.gbnf");

            let mut sampler = LlamaSampler::chain_simple([
                LlamaSampler::temp(TEMP),
                LlamaSampler::grammar(model, grammar, "root")
                    .expect("Failed to load grammar - check GBNF syntax"),
                LlamaSampler::dist(0),
            ]);

            let max_tokens = 1000;
            let mut n_cur = batch.n_tokens();
            let mut generated_tokens = Vec::new();
            let mut response_bytes = Vec::new();

            for _ in 0..max_tokens {
                let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);

                if model.is_eog_token(new_token) {
                    break;
                }

                generated_tokens.push(new_token);

                // Try to convert token to string for display (allow incomplete UTF-8)
                if let Ok(output) = model.token_to_str(new_token, Special::Tokenize) {
                    response_bytes.extend_from_slice(output.as_bytes());
                    // Use lossy conversion for real-time display
                    print!("{}", String::from_utf8_lossy(output.as_bytes()));
                    std::io::stdout().flush().unwrap();
                }

                batch.clear();
                batch.add(new_token, n_cur, &[0], true)?;
                n_cur += 1;

                ctx.decode(&mut batch)?;
            }

            println!();

            // Convert all bytes to string (lossy to handle any remaining incomplete sequences)
            let response = String::from_utf8_lossy(&response_bytes).to_string();
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
    let llm_result = get_response_from_llm(
        env.llm.as_ref(),
        &env.base_prompt_state,
        &msg,
        &summary,
        &previous_tool_calls,
    )
    .await;

    eprintln!("[DEBUG] llm_result: {:#?}", llm_result);

    match llm_result {
        Ok(llm_response) => {
            UserAction::LLMDecisionResult(Ok((llm_response.updated_summary, llm_response.outcome)))
        }
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
