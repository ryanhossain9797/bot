use crate::externals::long_term_memory_external::commit_to_memory;
use crate::externals::{
    llama_cpp_external::get_llm_decision, message_external::send_message,
    tool_call_external::execute_tool,
};
use crate::{
    externals::recall_short_term_external::execute_recall,
    models::user::{
        FunctionCall, HistoryEntry, LLMDecisionType, LLMInput, RecentConversation, User,
        UserAction, UserId, UserState,
    },
    Env, ENV,
};
use chrono::{Duration as ChronoDuration, Utc};
use framework::{
    new_state_machine, ExternalOperation, Schedule, Scheduled, Transition, TransitionResult,
};
use once_cell::sync::Lazy;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

fn handle_outcome(
    env: Arc<Env>,
    is_timeout: bool,
    outcome: LLMDecisionType,
    recent_conversation: RecentConversation,
    pending: Vec<String>,
) -> UserTransitionResult {
    match outcome {
        LLMDecisionType::Final { .. } => Ok((
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
        LLMDecisionType::IntermediateToolCall { tool_call, .. } => {
            let mut external = Vec::<UserExternalOperation>::new();
            external.push(Box::pin(execute_tool(
                env,
                tool_call,
                recent_conversation.history.clone(),
            )));

            Ok((
                User {
                    state: UserState::RunningTool {
                        is_timeout,
                        recent_conversation,
                    },
                    last_transition: Utc::now(),
                    pending,
                },
                external,
            ))
        }
        LLMDecisionType::InternalFunctionCall { function_call, .. } => {
            let mut external = Vec::<UserExternalOperation>::new();

            match function_call {
                FunctionCall::RecallShortTerm { .. } => {
                    external.push(Box::pin(execute_recall(
                        env.clone(),
                        recent_conversation.history.clone(),
                    )));
                }
            }

            Ok((
                User {
                    state: UserState::RunningInternalFunction {
                        is_timeout,
                        recent_conversation,
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
                    recent_conversation,
                    current_input,
                },
                UserAction::LLMDecisionResult(res),
            ) => match res {
                Ok(outcome) => {
                    // Add the input and output to history
                    let mut updated_history = recent_conversation.history;
                    updated_history.push(HistoryEntry::Input(current_input));
                    updated_history.push(HistoryEntry::Output(outcome.clone()));

                    let updated_conversation = RecentConversation {
                        history: updated_history,
                    };

                    // Extract message to send from outcome
                    let message_to_send = match &outcome {
                        LLMDecisionType::Final { response } => Some(response.clone()),
                        LLMDecisionType::InternalFunctionCall { .. } => None,
                        LLMDecisionType::IntermediateToolCall {
                            progress_notification,
                            ..
                        } => progress_notification.clone(),
                    };

                    // If there's a message to send, go to SendingMessage state
                    // Otherwise (silent tool call), go directly to RunningTool
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
                                        outcome: outcome.clone(),
                                        recent_conversation: updated_conversation,
                                    },
                                    last_transition: Utc::now(),
                                    ..user
                                },
                                external,
                            ))
                        }
                        None => handle_outcome(
                            env.clone(),
                            is_timeout,
                            outcome.clone(),
                            updated_conversation,
                            user.pending,
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
                },
                UserAction::MessageSent(_res),
            ) => handle_outcome(
                env.clone(),
                is_timeout,
                outcome,
                recent_conversation,
                user.pending,
            ),
            (
                UserState::RunningInternalFunction {
                    recent_conversation,
                    is_timeout,
                },
                UserAction::InternalFunctionResult(res),
            ) => {
                match res {
                    Ok(internal_function_result) => {
                        let current_input =
                            LLMInput::InternalFunctionResult(internal_function_result.clone());

                        // Function execution complete - get next LLM decision with function results
                        let mut external = Vec::<UserExternalOperation>::new();
                        external.push(Box::pin(get_llm_decision(
                            env.clone(),
                            current_input.clone(),
                            recent_conversation.history.clone(),
                            true,
                        )));

                        Ok((
                            User {
                                state: UserState::AwaitingLLMDecision {
                                    is_timeout,
                                    recent_conversation,
                                    current_input,
                                },
                                last_transition: Utc::now(),
                                ..user
                            },
                            external,
                        ))
                    }
                    Err(error_msg) => {
                        let error_result =
                            format!("Internal function execution failed: {}", error_msg);
                        let current_input = LLMInput::InternalFunctionResult(error_result);

                        // Let LLM handle the error and inform the user
                        let mut external = Vec::<UserExternalOperation>::new();
                        external.push(Box::pin(get_llm_decision(
                            env.clone(),
                            current_input.clone(),
                            recent_conversation.history.clone(),
                            true,
                        )));

                        Ok((
                            User {
                                state: UserState::AwaitingLLMDecision {
                                    is_timeout,
                                    recent_conversation,
                                    current_input,
                                },
                                last_transition: Utc::now(),
                                ..user
                            },
                            external,
                        ))
                    }
                }
            }
            (
                UserState::RunningTool {
                    recent_conversation,
                    is_timeout,
                },
                UserAction::ToolResult(res),
            ) => {
                match res {
                    Ok(tool_result) => {
                        let current_input = LLMInput::ToolResult(tool_result.clone());

                        // Tool execution complete - get next LLM decision with tool results
                        let mut external = Vec::<UserExternalOperation>::new();
                        external.push(Box::pin(get_llm_decision(
                            env.clone(),
                            current_input.clone(),
                            recent_conversation.history.clone(),
                            true,
                        )));

                        Ok((
                            User {
                                state: UserState::AwaitingLLMDecision {
                                    is_timeout,
                                    recent_conversation,
                                    current_input,
                                },
                                last_transition: Utc::now(),
                                ..user
                            },
                            external,
                        ))
                    }
                    Err(error_msg) => {
                        let error_result = format!("Tool execution failed: {}", error_msg);
                        let current_input = LLMInput::ToolResult(error_result);

                        // Let LLM handle the error and inform the user
                        let mut external = Vec::<UserExternalOperation>::new();
                        external.push(Box::pin(get_llm_decision(
                            env.clone(),
                            current_input.clone(),
                            recent_conversation.history.clone(),
                            true,
                        )));

                        Ok((
                            User {
                                state: UserState::AwaitingLLMDecision {
                                    is_timeout,
                                    recent_conversation,
                                    current_input,
                                },
                                last_transition: Utc::now(),
                                ..user
                            },
                            external,
                        ))
                    }
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
                    recent_conversation.history.clone(),
                    true,
                )));

                Ok((
                    User {
                        state: UserState::AwaitingLLMDecision {
                            is_timeout: true,
                            recent_conversation,
                            current_input,
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

fn post_transition(
    env: Arc<Env>,
    user_id: UserId,
    result: UserTransitionResult,
) -> UserTransitionResult {
    let (user, mut external) = result?;

    match (&user.state, user.pending.len() > 0) {
        (
            UserState::Idle {
                recent_conversation: last_conversation,
            },
            true,
        ) => {
            let recent_conversation = match last_conversation {
                Some((conv, _)) => conv.clone(),
                None => RecentConversation {
                    history: Vec::new(),
                },
            };

            let msg = user.pending.join("\n");

            let current_input = LLMInput::UserMessage(msg.clone());

            external.push(Box::pin(get_llm_decision(
                env.clone(),
                current_input.clone(),
                recent_conversation.history.clone(),
                false,
            )));

            let user = User {
                state: UserState::AwaitingLLMDecision {
                    is_timeout: false,
                    recent_conversation,
                    current_input,
                },
                last_transition: Utc::now(),
                pending: Vec::new(),
            };

            println!("Id: {0} {1:?}", user_id, user.state);

            Ok((user, external))
        }
        _ => Ok((user, external)),
    }
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
        | UserState::RunningInternalFunction { .. }
        | UserState::RunningTool { .. } => schedules.push(Scheduled {
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
