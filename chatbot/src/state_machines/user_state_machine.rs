use std::{future::Future, pin::Pin, sync::Arc};

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

use crate::externals::{
    llama_cpp_external::get_llm_decision, message_external::send_message,
    tool_call_external::execute_tool,
};

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

/// Handle the outcome after a message is sent (or skipped for silent tool calls).
/// Returns the next state transition based on the outcome.
fn handle_outcome(
    env: Arc<Env>,
    is_timeout: bool,
    outcome: LLMDecisionType,
    recent_conversation: RecentConversation,
) -> UserTransitionResult {
    match outcome {
        LLMDecisionType::Final { .. } => {
            // Final response sent - transition to Idle
            Ok((
                User {
                    state: UserState::Idle {
                        recent_conversation: if is_timeout {
                            None
                        } else {
                            Some((recent_conversation, Utc::now()))
                        },
                    },
                    last_transition: Utc::now(),
                },
                Vec::new(),
            ))
        }
        LLMDecisionType::IntermediateToolCall { tool_call, .. } => {
            // Intermediate message sent - now execute the tool
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
        match (user.state, action) {
            (_, UserAction::ForceReset) => Ok((
                User {
                    state: UserState::default(),
                    last_transition: Utc::now(),
                },
                Vec::new(),
            )),
            (
                UserState::Idle {
                    recent_conversation: last_conversation,
                },
                UserAction::NewMessage {
                    msg,
                    start_conversation: true,
                },
            ) => {
                let mut external = Vec::<UserExternalOperation>::new();

                let recent_conversation = match last_conversation {
                    Some((conv, _)) => conv,
                    None => RecentConversation {
                        history: Vec::new(),
                    },
                };

                let current_input = LLMInput::UserMessage(msg.clone());

                external.push(Box::pin(get_llm_decision(
                    env.clone(),
                    current_input.clone(),
                    recent_conversation.history.clone(),
                )));

                let user = User {
                    state: UserState::AwaitingLLMDecision {
                        is_timeout: false,
                        recent_conversation,
                        current_input,
                    },
                    last_transition: Utc::now(),
                };

                println!("Id: {0} {1:?}", user_id.1, user.state);

                Ok((user, external))
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
                                },
                                external,
                            ))
                        }
                        None => {
                            // Silent tool call - go directly to handle outcome
                            handle_outcome(
                                env.clone(),
                                is_timeout,
                                outcome.clone(),
                                updated_conversation,
                            )
                        }
                    }
                }
                Err(_) => Ok((
                    User {
                        state: UserState::Idle {
                            recent_conversation: None,
                        },
                        last_transition: Utc::now(),
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
            ) => {
                // Ignore errors from message sending - continue with normal flow regardless
                // Message sent (or failed, but we don't care) - check outcome to determine next state
                handle_outcome(env.clone(), is_timeout, outcome, recent_conversation)
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
                        )));

                        Ok((
                            User {
                                state: UserState::AwaitingLLMDecision {
                                    is_timeout,
                                    recent_conversation,
                                    current_input,
                                },
                                last_transition: Utc::now(),
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
                        )));

                        Ok((
                            User {
                                state: UserState::AwaitingLLMDecision {
                                    is_timeout,
                                    recent_conversation,
                                    current_input,
                                },
                                last_transition: Utc::now(),
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

                let timeout_message = "User said goodbye, RESPOND WITH GOODBYE BUT MENTION RELEVANT THINGS ABOUT THE CONVERSATION".to_string();
                let current_input = LLMInput::UserMessage(timeout_message);

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(get_llm_decision(
                    env.clone(),
                    current_input.clone(),
                    recent_conversation.history.clone(),
                )));

                Ok((
                    User {
                        state: UserState::AwaitingLLMDecision {
                            is_timeout: true,
                            recent_conversation,
                            current_input,
                        },
                        last_transition: Utc::now(),
                    },
                    external,
                ))
            }
            _ => Err(anyhow::anyhow!("Invalid state or action")),
        }
    })
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
