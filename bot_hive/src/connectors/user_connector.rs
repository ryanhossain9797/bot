use std::{num::NonZeroU32, sync::Arc, io::Write};

use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
};
use serde::Deserialize;
use serenity::all::CreateMessage;

use crate::{
    models::user::{UserAction, UserChannel, UserId},
    Env,
};

#[derive(Deserialize)]
struct LlmResponse {
    updated_summary: String,
    response: String,
}

async fn get_response_from_llm(
    llm: &(LlamaModel, LlamaBackend),
    msg: &str,
    summary: &str,
) -> anyhow::Result<LlmResponse> {
    let (model, backend) = llm;
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(2048)) // Context size
        .with_n_threads(num_cpus::get() as i32) // Use all CPU cores
        .with_n_threads_batch(num_cpus::get() as i32);

    let ctx_result = model.new_context(backend, ctx_params);

    match ctx_result {
        Ok(mut ctx) => {
            let conversation_prompt = if summary.is_empty() {
                format!(
                    "<|im_start|>system\nYou are a conversational assistant that can also control smart devices. Respond with ONLY a JSON object with this exact structure:

{{
  \"updated_summary\": \"Your updated summary of the conversation context\",
  \"response\": \"Your response to the user\",
  \"intent\": {{
    \"BasicConversation\": {{}} or null,
    \"ControlDevice\": {{device, property, value}} or null
  }}
}}

FIELD DESCRIPTIONS:
- updated_summary: CRITICAL - This is for YOUR OWN future reference, NOT for humans to read. Use an EXTREMELY COMPACT machine-readable format like abbreviated keys, symbols, or shorthand notation. Examples: 'usr:greet|dev:AC>temp=27|lights=on' or 'IMPT[AC_pref=cool]|recent:lights_on,temp_27' or 'ctx(polite=T,AC=27C)'. Prioritize: (1) important context - keep indefinitely, (2) trivial details - prioritize recent, drop old. Do NOT use full sentences - use the most compact format possible.
- response: Your direct response to the user's message.
- intent: Exactly ONE intent must be non-null (oneof/enum pattern).

INTENT RULES (oneof pattern - exactly ONE intent must be non-null):
1. BasicConversation: For general conversation, questions, greetings. Set to {{}} (empty object) when active, null otherwise.
2. ControlDevice: For device control commands. Contains {{\"device\": \"name\", \"property\": \"property\", \"value\": value}} when active, null otherwise.

EXAMPLES:

User: \"Hello!\"
{{\"updated_summary\":\"usr:greet\",\"response\":\"Hello! How can I help you today?\",\"intent\":{{\"BasicConversation\":{{}},\"ControlDevice\":null}}}}

User: \"Set AC to 27 degrees\"
{{\"updated_summary\":\"dev:AC>temp=27\",\"response\":\"Setting AC temperature to 27 degrees\",\"intent\":{{\"BasicConversation\":null,\"ControlDevice\":{{\"device\":\"AC\",\"property\":\"temperature\",\"value\":\"27\"}}}}}}

User: \"What's the weather like?\"
{{\"updated_summary\":\"q:weather(N/A)\",\"response\":\"I don't have access to weather information, but I can help you control your devices!\",\"intent\":{{\"BasicConversation\":{{}},\"ControlDevice\":null}}}}

User: \"Turn on the lights\"
{{\"updated_summary\":\"dev:light>pwr=on\",\"response\":\"Turning on the lights now\",\"intent\":{{\"BasicConversation\":null,\"ControlDevice\":{{\"device\":\"light\",\"property\":\"power\",\"value\":\"on\"}}}}}}

Keep responses concise (a few sentences or less) unless the user asks for more detail.
Respond ONLY with valid JSON, no additional text.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
                    msg
                )
            } else {
                format!(
                    "<|im_start|>system\nYou are a conversational assistant that can also control smart devices. Respond with ONLY a JSON object with this exact structure:
d
{{
  \"updated_summary\": \"Your updated summary of the conversation context\",
  \"response\": \"Your response to the user\",
  \"intent\": {{
    \"BasicConversation\": {{}} or null,
    \"ControlDevice\": {{device, property, value}} or null
  }}
}}

FIELD DESCRIPTIONS:
- updated_summary: CRITICAL - This is for YOUR OWN future reference, NOT for humans to read. Use an EXTREMELY COMPACT machine-readable format like abbreviated keys, symbols, or shorthand notation. Examples: 'usr:greet|dev:AC>temp=27|lights=on' or 'IMPT[AC_pref=cool]|recent:lights_on,temp_27' or 'ctx(polite=T,AC=27C)'. Prioritize: (1) important context - keep indefinitely, (2) trivial details - prioritize recent, drop old. Do NOT use full sentences - use the most compact format possible.
- response: Your direct response to the user's message.
- intent: Exactly ONE intent must be non-null (oneof/enum pattern).

INTENT RULES (oneof pattern - exactly ONE intent must be non-null):
1. BasicConversation: For general conversation, questions, greetings. Set to {{}} (empty object) when active, null otherwise.
2. ControlDevice: For device control commands. Contains {{\"device\": \"name\", \"property\": \"property\", \"value\": value}} when active, null otherwise.

EXAMPLES:

User: \"Hello!\"
{{\"updated_summary\":\"usr:greet\",\"response\":\"Hello! How can I help you today?\",\"intent\":{{\"BasicConversation\":{{}},\"ControlDevice\":null}}}}

User: \"Set AC to 27 degrees\"
{{\"updated_summary\":\"dev:AC>temp=27\",\"response\":\"Setting AC temperature to 27 degrees\",\"intent\":{{\"BasicConversation\":null,\"ControlDevice\":{{\"device\":\"AC\",\"property\":\"temperature\",\"value\":\"27\"}}}}}}

User: \"What's the weather like?\"
{{\"updated_summary\":\"q:weather(N/A)\",\"response\":\"I don't have access to weather information, but I can help you control your devices!\",\"intent\":{{\"BasicConversation\":{{}},\"ControlDevice\":null}}}}

User: \"Turn on the lights\"
{{\"updated_summary\":\"dev:light>pwr=on\",\"response\":\"Turning on the lights now\",\"intent\":{{\"BasicConversation\":null,\"ControlDevice\":{{\"device\":\"light\",\"property\":\"power\",\"value\":\"on\"}}}}}}

Previous conversation summary:
{}

Based on the previous summary above, update it to reflect the new exchange. DO NOT USE FULL SENTENCES. Use compact notation with symbols/abbreviations (like 'usr:greet|dev:AC=27|lights:on'). Drop old trivial details, keep important context, prioritize recent for non-important items.
Keep responses concise (a few sentences or less) unless the user asks for more detail.
Respond ONLY with valid JSON, no additional text.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
                    summary, msg
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

            let parsed_response: LlmResponse = serde_json::from_str(&response)?;
            Ok(parsed_response)
        },
        Err(err) => Err(anyhow::anyhow!(err)),
    }
}

pub async fn handle_bot_message(env: Arc<Env>, user_id: UserId, msg: String, summary: String) -> UserAction {
    let user_id_result = match user_id.0 {
        UserChannel::Discord => {
            let user_id_result = user_id.1.parse::<u64>();
            match user_id_result {
                Ok(user_id) => Ok(serenity::all::UserId::new(user_id)),
                Err(err) => Err(anyhow::anyhow!(err)),
            }
        }
        _ => panic!("Telegram not yet implemented"),
    };
    match user_id_result {
        Err(err) => UserAction::SendResult(Arc::new(Err(err))),
        Ok(user_id) => {
            let dm_channel_result = match user_id.to_user(&env.discord_http).await {
                Ok(user) => user.create_dm_channel(&env.discord_http).await,
                Err(e) => Err(e),
            };

            match dm_channel_result {
                Ok(channel) => {
                    // Wrap LLM processing in a scope to ensure all non-Send types are dropped
                    let llm_result = get_response_from_llm(env.llm.as_ref(), &msg, &summary).await; // End of scope - all non-Send types are dropped here

                    // Now send the message after llama-cpp objects are dropped
                    match llm_result {
                        Ok(llm_response) => {
                            let res = channel
                                .send_message(
                                    &env.discord_http,
                                    CreateMessage::new().content(&llm_response.response),
                                )
                                .await;

                            match res {
                                Ok(_) => {
                                    // Return the LLM-managed summary
                                    UserAction::SendResult(Arc::new(Ok(llm_response.updated_summary)))
                                }
                                Err(err) => {
                                    UserAction::SendResult(Arc::new(Err(anyhow::anyhow!(err))))
                                }
                            }
                        }
                        Err(err) => UserAction::SendResult(Arc::new(Err(err))),
                    }
                }
                Err(err) => UserAction::SendResult(Arc::new(Err(anyhow::anyhow!(err)))),
            }
        }
    }
}
