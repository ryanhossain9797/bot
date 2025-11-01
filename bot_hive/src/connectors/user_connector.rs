use std::{num::NonZeroU32, sync::Arc};

use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_batch::LlamaBatch,
    model::{AddBos, Special},
    sampling::LlamaSampler,
};
use serenity::all::CreateMessage;

use crate::{
    models::user::{UserAction, UserChannel, UserId},
    Env,
};

pub async fn handle_bot_message(env: Arc<Env>, user_id: UserId, msg: String) -> UserAction {
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
                    let (model, backend) = env.llm.as_ref();

                    // Wrap LLM processing in a scope to ensure all non-Send types are dropped
                    let response = {
                        let ctx_params = LlamaContextParams::default()
                            .with_n_ctx(NonZeroU32::new(2048)) // Context size
                            .with_n_threads(num_cpus::get() as i32) // Use all CPU cores
                            .with_n_threads_batch(num_cpus::get() as i32);

                        let ctx_result = model.new_context(backend, ctx_params);

                        match ctx_result {
                            Ok(mut ctx) => {
                                let conversation_prompt = format!(
                                    "<|im_start|>system\nYou are a very basic conversational agent\n

                    Respond in a few words, no rambling.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
                    msg
                                );
                                let tokens = model
                                    .str_to_token(&conversation_prompt, AddBos::Always)
                                    .unwrap();

                                // Create a batch and add tokens
                                let mut batch = LlamaBatch::new(512, 1);

                                for (i, token) in tokens.iter().enumerate() {
                                    let is_last = i == tokens.len() - 1;
                                    batch.add(*token, i as i32, &[0], is_last).unwrap();
                                }

                                // Process the prompt
                                ctx.decode(&mut batch).unwrap();

                                // Create sampler chain with grammar constraint for structured JSON output
                                let mut sampler = LlamaSampler::chain_simple([
                                    LlamaSampler::temp(0.3),
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
                                    let output =
                                        model.token_to_str(new_token, Special::Tokenize).unwrap();

                                    response.push_str(&output);

                                    // Stop if we hit the ChatML end token or completed a valid JSON object
                                    if response.contains("<|im_end|>")
                                        || output.contains("<|im_end|>")
                                    {
                                        break;
                                    }

                                    // Prepare next batch
                                    batch.clear();
                                    batch.add(new_token, n_cur, &[0], true).unwrap();
                                    n_cur += 1;

                                    // Decode
                                    ctx.decode(&mut batch).unwrap();
                                }

                                // Drop ctx, batch, and sampler before any await points
                                Ok(response)
                            }
                            Err(err) => Err(anyhow::anyhow!(err)),
                        }
                    }; // End of scope - all non-Send types are dropped here

                    // Now send the message after llama-cpp objects are dropped
                    match response {
                        Ok(response_text) => {
                            let res = channel
                                .send_message(
                                    &env.discord_http,
                                    CreateMessage::new().content(response_text),
                                )
                                .await;

                            match res {
                                Ok(_) => UserAction::SendResult(Arc::new(Ok(()))),
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
