use crate::externals::long_term_memory_external::commit_to_memory;
use crate::externals::{
    llama_cpp_external::get_llm_decision, message_external::send_message,
    tool_call_external::execute_tool,
};
use crate::models::user::{LLMResponse, ToolResult, ToolResultData, MAX_TOOL_ROUNDS};
use crate::{
    models::user::{
        HistoryEntry, LLMDecisionType, LLMInput, RecentConversation, User, UserAction, UserId,
        UserState,
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

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

fn handle_outcome(
    env: Arc<Env>,
    user_id: &UserId,
    is_timeout: bool,
    response: LLMResponse,
    recent_conversation: RecentConversation,
    pending: Vec<String>,
    tool_rounds: usize,
) -> UserTransitionResult {
    match response.output {
        LLMDecisionType::MessageUser { .. } => Ok((
            User {
                state: UserState::Idle {
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
        )),
        LLMDecisionType::IntermediateToolCall { tool_calls, .. } => {
            // Any preamble `message` was already sent on the way into SendingMessage; here we only
            // dispatch the calls. Dispatch every call as its own external op and seed pending_tools, so each returning
            // result can be moved to completed_tools by id (one round per batch).
            let mut external = Vec::<UserExternalOperation>::new();
            let mut pending_tools = HashMap::new();
            for tool_call in tool_calls {
                external.push(Box::pin(execute_tool(
                    env.clone(),
                    tool_call.clone(),
                    user_id.to_string(),
                    recent_conversation.history.clone(),
                )));
                pending_tools.insert(tool_call.id.clone(), tool_call);
            }

            Ok((
                User {
                    state: UserState::RunningTools {
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
}

pub fn user_transition(
    env: Arc<Env>,
    user_id: UserId,
    user: User,
    action: &UserAction,
) -> Pin<Box<dyn Future<Output = UserTransitionResult> + Send + '_>> {
    Box::pin(async move {
        let state = match (user.state, action) {
            (_, UserAction::ForceReset) => Ok((
                User {
                    pending: Vec::new(),
                    state: UserState::default(),
                    last_transition: Utc::now(),
                },
                Vec::new(),
            )),
            (
                user_state,
                UserAction::NewMessage {
                    msg,
                    start_conversation,
                },
            ) => {
                let accept_message = match (&user_state, start_conversation) {
                    (
                        UserState::Idle {
                            recent_conversation: None,
                        },
                        false,
                    ) => false,
                    _ => true,
                };

                let pending = if accept_message {
                    let mut pending = user.pending;
                    pending.push(msg.clone());
                    pending
                } else {
                    user.pending
                };

                Ok((
                    User {
                        pending,
                        state: user_state,
                        ..user
                    },
                    Vec::new(),
                ))
            }
            (
                UserState::AwaitingLLMDecision {
                    is_timeout,
                    history,
                    current_input,
                    tool_rounds,
                },
                UserAction::LLMDecisionResult(res),
            ) => match res {
                Ok(response) => {
                    // Add the input and output to history
                    let mut updated_history = history;
                    updated_history.push(HistoryEntry::Input(current_input));
                    updated_history.push(HistoryEntry::Output(response.clone()));

                    let updated_conversation = RecentConversation {
                        thoughts: response.thoughts.clone(),
                        history: updated_history,
                    };

                    // Extract message to send from outcome: a plain reply, or — for a tool-call
                    // outcome — a model preamble if one was emitted (rare), else a fixed
                    // "Using tool: <name>" notice when the feature is enabled, so the user isn't
                    // left in silence. Disabled → None → silent tool turn. Either way it goes
                    // through SendingMessage; the held `outcome` still carries the calls to
                    // dispatch on MessageSent.
                    let message_to_send = match &response.output {
                        LLMDecisionType::MessageUser { response } => Some(response.clone()),
                        LLMDecisionType::IntermediateToolCall { tool_calls, message } => {
                            message.clone().or_else(|| {
                                env.announce_tool_use.then(|| {
                                    tool_calls
                                        .iter()
                                        .map(|tc| format!("Using tool: {}", tc.tool_type.wire_name()))
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                            })
                        }
                    };

                    match message_to_send {
                        Some(message) => {
                            // Transition to SendingMessage state and trigger message sending
                            let mut external = Vec::<UserExternalOperation>::new();
                            external.push(Box::pin(send_message(
                                env.clone(),
                                user_id.clone(),
                                message,
                            )));

                            Ok((
                                User {
                                    state: UserState::SendingMessage {
                                        is_timeout,
                                        outcome: response.clone(),
                                        recent_conversation: updated_conversation,
                                        tool_rounds,
                                    },
                                    last_transition: Utc::now(),
                                    ..user
                                },
                                external,
                            ))
                        }
                        None => handle_outcome(
                            env.clone(),
                            &user_id,
                            is_timeout,
                            response.clone(),
                            updated_conversation,
                            user.pending,
                            tool_rounds,
                        ),
                    }
                }
                Err(_) => Ok((
                    User {
                        state: UserState::Idle {
                            recent_conversation: None,
                        },
                        last_transition: Utc::now(),
                        ..user
                    },
                    Vec::new(),
                )),
            },
            (
                UserState::SendingMessage {
                    is_timeout,
                    outcome,
                    recent_conversation,
                    tool_rounds,
                },
                UserAction::MessageSent(_res),
            ) => handle_outcome(
                env.clone(),
                &user_id,
                is_timeout,
                outcome,
                recent_conversation,
                user.pending,
                tool_rounds,
            ),
            (
                UserState::RunningTools {
                    recent_conversation,
                    is_timeout,
                    tool_rounds,
                    mut pending_tools,
                    mut completed_tools,
                },
                UserAction::ToolResult { id, result },
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
                    let mut pending = user.pending;
                    let current_input = LLMInput::ToolResults(results, take_pending(&mut pending));

                    let history = recent_conversation.history();
                    let mut external = Vec::<UserExternalOperation>::new();
                    external.push(Box::pin(get_llm_decision(
                        env.clone(),
                        current_input.clone(),
                        Some(recent_conversation),
                        tool_rounds,
                        MAX_TOOL_ROUNDS,
                    )));

                    Ok((
                        User {
                            state: UserState::AwaitingLLMDecision {
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
                        User {
                            state: UserState::RunningTools {
                                is_timeout,
                                recent_conversation,
                                tool_rounds,
                                pending_tools,
                                completed_tools,
                            },
                            last_transition: Utc::now(),
                            ..user
                        },
                        Vec::new(),
                    ))
                }
            }
            (
                UserState::Idle {
                    recent_conversation: Some((recent_conversation, _)),
                },
                UserAction::Timeout,
            ) => {
                println!("Timed Out");

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(commit_to_memory(
                    Arc::clone(&env),
                    user_id.to_string(),
                    recent_conversation.history.clone(),
                )));

                Ok((
                    User {
                        state: UserState::CommitingToMemory {
                            recent_conversation,
                        },
                        last_transition: Utc::now(),
                        ..user
                    },
                    external,
                ))
            }
            (
                UserState::CommitingToMemory {
                    recent_conversation,
                },
                UserAction::CommitResult(_),
            ) => {
                println!("Commited to Memory");

                let timeout_message = "User said goodbye, RESPOND WITH GOODBYE BUT MENTION RELEVANT THINGS ABOUT THE CONVERSATION".to_string();
                let current_input = LLMInput::UserMessage(timeout_message);

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(get_llm_decision(
                    env.clone(),
                    current_input.clone(),
                    Some(recent_conversation.clone()),
                    0,
                    MAX_TOOL_ROUNDS,
                )));

                Ok((
                    User {
                        state: UserState::AwaitingLLMDecision {
                            is_timeout: true,
                            history: recent_conversation.history,
                            current_input,
                            tool_rounds: 0,
                        },
                        last_transition: Utc::now(),
                        ..user
                    },
                    external,
                ))
            }
            _ => Err(anyhow::anyhow!("Invalid state or action")),
        };

        post_transition(env, user_id, state)
    })
}

/// Drain buffered user messages into a single newline-joined string, clearing `pending`. Returns
/// `None` if there was nothing buffered. Single source of truth for both pending drain points (the
/// Idle drain below and the mid-tool-loop fold in the RunningTools branch), so they stay identical.
fn take_pending(pending: &mut Vec<String>) -> Option<String> {
    (!pending.is_empty()).then(|| std::mem::take(pending).join("\n"))
}

fn post_transition(
    env: Arc<Env>,
    _user_id: UserId,
    result: UserTransitionResult,
) -> UserTransitionResult {
    let (mut user, mut external) = result?;

    // Never rest in Idle with buffered input: drain it into a fresh user turn. (Mirrors the
    // mid-tool-loop fold in the RunningTools branch, which folds into a tool-result turn instead.)
    let recent_conversation = match &user.state {
        UserState::Idle {
            recent_conversation,
        } => recent_conversation.clone(),
        _ => return Ok((user, external)),
    };

    let Some(msg) = take_pending(&mut user.pending) else {
        return Ok((user, external));
    };

    let history = recent_conversation
        .as_ref()
        .map(|(rc, _)| rc.history())
        .unwrap_or_else(Vec::new);

    let current_input = LLMInput::UserMessage(msg);

    external.push(Box::pin(get_llm_decision(
        env.clone(),
        current_input.clone(),
        recent_conversation.map(|(rc, _)| rc),
        0,
        MAX_TOOL_ROUNDS,
    )));

    Ok((
        User {
            state: UserState::AwaitingLLMDecision {
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

pub fn schedule(user: &User) -> Vec<Scheduled<UserAction>> {
    let mut schedules = Vec::new();
    match user.state {
        UserState::Idle {
            recent_conversation: Some((_, last_activity)),
        } => schedules.push(Scheduled {
            at: last_activity + ChronoDuration::milliseconds(300_000),
            action: UserAction::Timeout,
        }),
        UserState::AwaitingLLMDecision { .. }
        | UserState::SendingMessage { .. }
        | UserState::CommitingToMemory { .. }
        | UserState::RunningTools { .. } => schedules.push(Scheduled {
            at: user.last_transition + ChronoDuration::milliseconds(600_000),
            action: UserAction::ForceReset,
        }),
        _ => {}
    }

    schedules
}

pub static USER_STATE_MACHINE: Lazy<framework::StateMachineHandle<UserId, UserAction>> =
    Lazy::new(|| {
        new_state_machine(
            ENV.get().expect("ENV not initialized").clone(),
            Transition(user_transition),
            Schedule(schedule),
        )
    });
