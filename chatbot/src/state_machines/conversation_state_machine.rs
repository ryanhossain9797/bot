use crate::externals::long_term_memory_external::commit_to_memory;
use crate::externals::{
    llama_cpp_external::get_llm_decision, message_external::send_message,
    tool_call_external::execute_tool,
};
use crate::types::conversation::{
    last_conversation_message, LLMResponse, ToolResult, ToolResultData, MAX_TOOL_ROUNDS,
};
use crate::{
    types::conversation::{
        HistoryEntry, LLMInput, RecentConversation, Conversation, ConversationAction, ConversationId, ConversationMessage,
        ConversationState,
    },
    Env, ENV,
};
use chrono::{Duration as ChronoDuration, Utc};
use framework::{
    new_state_machine, ExternalOperation, Schedule, Scheduled, Transition, TransitionResult,
};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type ConversationTransitionResult = TransitionResult<Conversation, ConversationAction>;
type ConversationExternalOperation = ExternalOperation<ConversationAction>;

fn handle_outcome(
    env: Arc<Env>,
    conversation_id: &ConversationId,
    is_timeout: bool,
    response: LLMResponse,
    recent_conversation: RecentConversation,
    pending: Vec<ConversationMessage>,
    tool_rounds: usize,
) -> ConversationTransitionResult {
    // Any `message` was already sent on the way into SendingMessage; here we act on the tool calls.
    if response.tool_calls.is_empty() {
        // No tools to run — settle into Idle.
        Ok((
            Conversation {
                state: ConversationState::Idle {
                    recent_conversation: if is_timeout {
                        None
                    } else {
                        Some((recent_conversation, Utc::now()))
                    },
                },
                last_transition: Utc::now(),
                pending,
            },
            Vec::new(),
        ))
    } else {
        // Dispatch every call as its own external op and seed pending_tools, so each returning
        // result can be moved to completed_tools by id (one round per batch).
        let mut external = Vec::<ConversationExternalOperation>::new();
        let mut pending_tools = HashMap::new();
        for tool_call in response.tool_calls {
            external.push(Box::pin(execute_tool(
                env.clone(),
                tool_call.clone(),
                conversation_id.to_string(),
                recent_conversation.history.clone(),
            )));
            pending_tools.insert(tool_call.id.clone(), tool_call);
        }

        Ok((
            Conversation {
                state: ConversationState::RunningTools {
                    is_timeout,
                    recent_conversation,
                    tool_rounds: tool_rounds + 1,
                    pending_tools,
                    completed_tools: Vec::new(),
                },
                last_transition: Utc::now(),
                pending,
            },
            external,
        ))
    }
}

pub fn conversation_transition(
    env: Arc<Env>,
    conversation_id: ConversationId,
    conversation: Conversation,
    action: &ConversationAction,
) -> Pin<Box<dyn Future<Output = ConversationTransitionResult> + Send + '_>> {
    Box::pin(async move {
        let state = match (conversation.state, action) {
            (_, ConversationAction::ForceReset) => Ok((
                Conversation {
                    pending: Vec::new(),
                    state: ConversationState::default(),
                    last_transition: Utc::now(),
                },
                Vec::new(),
            )),
            (
                user_state,
                ConversationAction::NewMessage {
                    msg,
                    user_id,
                    name,
                    is_group,
                    bot_identity,
                },
            ) => {
                // Every message is accepted and buffered (the old `start_conversation` mention-gate
                // is gone — in a group the model itself decides whether to reply; see the group
                // system prompt). Queued iff it arrived while the bot was busy (any non-Idle state),
                // so it crossed an in-flight response; an Idle arrival drains immediately and isn't.
                let mut pending = conversation.pending;
                let queued = !matches!(&user_state, ConversationState::Idle { .. });
                pending.push(ConversationMessage {
                    text: msg.clone(),
                    queued,
                    user_id: user_id.clone(),
                    name: name.clone(),
                    is_group: *is_group,
                    bot_identity: bot_identity.clone(),
                });

                Ok((
                    Conversation {
                        pending,
                        state: user_state,
                        ..conversation
                    },
                    Vec::new(),
                ))
            }
            (
                ConversationState::AwaitingLLMDecision {
                    is_timeout,
                    history,
                    current_input,
                    tool_rounds,
                },
                ConversationAction::LLMDecisionResult(res),
            ) => match res {
                Ok(response) => {
                    // Add the input and output to history. An empty decision (no message, no tool
                    // calls) is still recorded; it renders as `content: ""` via to_openai_message.
                    let mut updated_history = history;
                    updated_history.push(HistoryEntry::Input(current_input));
                    updated_history.push(HistoryEntry::Output(response.clone()));

                    let updated_conversation = RecentConversation {
                        thoughts: response.thoughts.clone(),
                        history: updated_history,
                    };

                    // What to send this turn: the model's message if it produced one; otherwise,
                    // when the turn dispatches tools and the feature is on, a fixed "Using tool:
                    // <name>" notice so the user isn't left in silence. None → nothing to send
                    // (silent tool turn, or a do-nothing turn). Either way, if the held `outcome`
                    // carries tool calls they're dispatched after MessageSent via handle_outcome.
                    let message_to_send = response.message.clone().or_else(|| {
                        (!response.tool_calls.is_empty() && env.announce_tool_use).then(|| {
                            // Discord subtext (`-# `) + italic so it reads as a small, greyed
                            // status line, not a real bot message. One tool → singular; several →
                            // a single combined line.
                            let names: Vec<&str> = response
                                .tool_calls
                                .iter()
                                .map(|tc| tc.tool_type.wire_name())
                                .collect();
                            match names.as_slice() {
                                [one] => format!("-# *using tool: {one}*"),
                                many => format!("-# *using multiple tools: {}*", many.join(", ")),
                            }
                        })
                    });

                    match message_to_send {
                        Some(message) => {
                            // Transition to SendingMessage state and trigger message sending
                            let mut external = Vec::<ConversationExternalOperation>::new();
                            external.push(Box::pin(send_message(
                                env.clone(),
                                conversation_id.clone(),
                                message,
                            )));

                            Ok((
                                Conversation {
                                    state: ConversationState::SendingMessage {
                                        is_timeout,
                                        outcome: response.clone(),
                                        recent_conversation: updated_conversation,
                                        tool_rounds,
                                    },
                                    last_transition: Utc::now(),
                                    ..conversation
                                },
                                external,
                            ))
                        }
                        None => handle_outcome(
                            env.clone(),
                            &conversation_id,
                            is_timeout,
                            response.clone(),
                            updated_conversation,
                            conversation.pending,
                            tool_rounds,
                        ),
                    }
                }
                Err(_) => Ok((
                    Conversation {
                        state: ConversationState::Idle {
                            recent_conversation: None,
                        },
                        last_transition: Utc::now(),
                        ..conversation
                    },
                    Vec::new(),
                )),
            },
            (
                ConversationState::SendingMessage {
                    is_timeout,
                    outcome,
                    recent_conversation,
                    tool_rounds,
                },
                ConversationAction::MessageSent(_res),
            ) => handle_outcome(
                env.clone(),
                &conversation_id,
                is_timeout,
                outcome,
                recent_conversation,
                conversation.pending,
                tool_rounds,
            ),
            (
                ConversationState::RunningTools {
                    recent_conversation,
                    is_timeout,
                    tool_rounds,
                    mut pending_tools,
                    mut completed_tools,
                },
                ConversationAction::ToolResult { id, result },
            ) => {
                // Move this tool from pending to completed, folding any error into the result data.
                if let Some(tool_call) = pending_tools.remove(id) {
                    let data = match result {
                        Ok(data) => data.clone(),
                        Err(error_msg) => {
                            let msg = format!("Tool execution failed: {error_msg}");
                            ToolResultData {
                                actual: msg.clone(),
                                simplified: msg,
                            }
                        }
                    };
                    completed_tools.push((tool_call, data));
                } else {
                    eprintln!("[warn] tool result for unknown id {id}; ignoring");
                }

                if pending_tools.is_empty() {
                    // Whole batch done: sort by id so calls and results align positionally, then
                    // hand the results back to the model as one tool-result turn.
                    completed_tools.sort_by(|(a, _), (b, _)| a.id.cmp(&b.id));
                    let results = completed_tools
                        .into_iter()
                        .map(|(tool_call, data)| ToolResult {
                            id: tool_call.id,
                            data,
                        })
                        .collect();
                    // Fold any messages the user sent mid-tool-run into this same turn (after the
                    // results, per the OpenAI protocol). Clearing pending here is required — else
                    // post_transition drains it again at Idle and the message is sent twice.
                    let mut pending = conversation.pending;
                    let current_input = LLMInput::ToolResults(results, take_pending(&mut pending));

                    let history = recent_conversation.history();
                    let mut external = Vec::<ConversationExternalOperation>::new();
                    external.push(Box::pin(get_llm_decision(
                        env.clone(),
                        current_input.clone(),
                        Some(recent_conversation),
                        tool_rounds,
                        MAX_TOOL_ROUNDS,
                    )));

                    Ok((
                        Conversation {
                            state: ConversationState::AwaitingLLMDecision {
                                is_timeout,
                                history,
                                current_input,
                                tool_rounds,
                            },
                            last_transition: Utc::now(),
                            pending,
                        },
                        external,
                    ))
                } else {
                    // Still waiting on other tools in the batch — stay put, dispatch nothing new.
                    Ok((
                        Conversation {
                            state: ConversationState::RunningTools {
                                is_timeout,
                                recent_conversation,
                                tool_rounds,
                                pending_tools,
                                completed_tools,
                            },
                            last_transition: Utc::now(),
                            ..conversation
                        },
                        Vec::new(),
                    ))
                }
            }
            (
                ConversationState::Idle {
                    recent_conversation: Some((recent_conversation, _)),
                },
                ConversationAction::Timeout,
            ) => {
                println!("Timed Out");

                let mut external = Vec::<ConversationExternalOperation>::new();

                external.push(Box::pin(commit_to_memory(
                    Arc::clone(&env),
                    conversation_id.to_string(),
                    recent_conversation.history.clone(),
                )));

                Ok((
                    Conversation {
                        state: ConversationState::CommitingToMemory {
                            recent_conversation,
                        },
                        last_transition: Utc::now(),
                        ..conversation
                    },
                    external,
                ))
            }
            (
                ConversationState::CommitingToMemory {
                    recent_conversation,
                },
                ConversationAction::CommitResult(_),
            ) => {
                println!("Commited to Memory");

                let timeout_message = "User said goodbye, RESPOND WITH GOODBYE BUT MENTION RELEVANT THINGS ABOUT THE CONVERSATION".to_string();
                // Synthetic system message (no real sender). Inherit the conversation's group-ness
                // and the bot's platform identity from the last real message so the goodbye uses the
                // right system prompt.
                let last = last_conversation_message(&recent_conversation.history);
                let current_input = LLMInput::ConversationMessage(ConversationMessage {
                    text: timeout_message,
                    queued: false,
                    user_id: String::new(),
                    name: String::new(),
                    is_group: last.map(|m| m.is_group).unwrap_or(false),
                    bot_identity: last.map(|m| m.bot_identity.clone()).unwrap_or_default(),
                });

                let mut external = Vec::<ConversationExternalOperation>::new();

                external.push(Box::pin(get_llm_decision(
                    env.clone(),
                    current_input.clone(),
                    Some(recent_conversation.clone()),
                    0,
                    MAX_TOOL_ROUNDS,
                )));

                Ok((
                    Conversation {
                        state: ConversationState::AwaitingLLMDecision {
                            is_timeout: true,
                            history: recent_conversation.history,
                            current_input,
                            tool_rounds: 0,
                        },
                        last_transition: Utc::now(),
                        ..conversation
                    },
                    external,
                ))
            }
            _ => Err(anyhow::anyhow!("Invalid state or action")),
        };

        post_transition(env, conversation_id, state)
    })
}

/// Drain buffered user messages into a single newline-joined string, clearing `pending`. Returns
/// `None` if there was nothing buffered. Single source of truth for both pending drain points (the
/// Idle drain below and the mid-tool-loop fold in the RunningTools branch), so they stay identical.
fn take_pending(pending: &mut Vec<ConversationMessage>) -> Option<ConversationMessage> {
    (!pending.is_empty()).then(|| {
        let drained = std::mem::take(pending);
        // Messages drained together are homogeneous (all queued during a busy turn, or a single
        // Idle arrival); mark the merged message queued if any were. Each member's `text` already
        // carries its own name prefix, so joining keeps per-speaker attribution even when a group
        // batch mixes senders. Identity fields take the last message's (best-effort for the merge).
        let queued = drained.iter().any(|m| m.queued);
        let is_group = drained.last().map(|m| m.is_group).unwrap_or(false);
        let user_id = drained.last().map(|m| m.user_id.clone()).unwrap_or_default();
        let name = drained.last().map(|m| m.name.clone()).unwrap_or_default();
        let bot_identity = drained.last().map(|m| m.bot_identity.clone()).unwrap_or_default();
        let text = drained
            .into_iter()
            .map(|m| m.text)
            .collect::<Vec<_>>()
            .join("\n");
        ConversationMessage {
            text,
            queued,
            user_id,
            name,
            is_group,
            bot_identity,
        }
    })
}

fn post_transition(
    env: Arc<Env>,
    _conversation_id: ConversationId,
    result: ConversationTransitionResult,
) -> ConversationTransitionResult {
    let (mut conversation, mut external) = result?;

    // Never rest in Idle with buffered input: drain it into a fresh user turn. (Mirrors the
    // mid-tool-loop fold in the RunningTools branch, which folds into a tool-result turn instead.)
    let recent_conversation = match &conversation.state {
        ConversationState::Idle {
            recent_conversation,
        } => recent_conversation.clone(),
        _ => return Ok((conversation, external)),
    };

    let Some(msg) = take_pending(&mut conversation.pending) else {
        return Ok((conversation, external));
    };

    let history = recent_conversation
        .as_ref()
        .map(|(rc, _)| rc.history())
        .unwrap_or_else(Vec::new);

    let current_input = LLMInput::ConversationMessage(msg);

    external.push(Box::pin(get_llm_decision(
        env.clone(),
        current_input.clone(),
        recent_conversation.map(|(rc, _)| rc),
        0,
        MAX_TOOL_ROUNDS,
    )));

    Ok((
        Conversation {
            state: ConversationState::AwaitingLLMDecision {
                is_timeout: false,
                history,
                current_input,
                tool_rounds: 0,
            },
            last_transition: Utc::now(),
            pending: Vec::new(),
        },
        external,
    ))
}

pub fn schedule(conversation: &Conversation) -> Vec<Scheduled<ConversationAction>> {
    let mut schedules = Vec::new();
    match conversation.state {
        ConversationState::Idle {
            recent_conversation: Some((_, last_activity)),
        } => schedules.push(Scheduled {
            at: last_activity + ChronoDuration::milliseconds(900_000),
            action: ConversationAction::Timeout,
        }),
        ConversationState::AwaitingLLMDecision { .. }
        | ConversationState::SendingMessage { .. }
        | ConversationState::CommitingToMemory { .. }
        | ConversationState::RunningTools { .. } => schedules.push(Scheduled {
            at: conversation.last_transition + ChronoDuration::milliseconds(600_000),
            action: ConversationAction::ForceReset,
        }),
        _ => {}
    }

    schedules
}

pub static CONVERSATION_STATE_MACHINE: Lazy<framework::StateMachineHandle<ConversationId, ConversationAction>> =
    Lazy::new(|| {
        new_state_machine(
            ENV.get().expect("ENV not initialized").clone(),
            Transition(conversation_transition),
            Schedule(schedule),
        )
    });
