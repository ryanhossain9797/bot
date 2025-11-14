use std::{future::Future, pin::Pin, sync::Arc};

use crate::{
    models::user::{MessageOutcome, RecentConversation, User, UserAction, UserId, UserState},
    Env, ENV,
};
use chrono::{Duration as ChronoDuration, Utc};
use lib_hive::{
    new_life_cycle, ExternalOperation, Schedule, Scheduled, Transition, TransitionResult,
};
use once_cell::sync::Lazy;

use crate::connectors::{
    llm_connector::get_llm_decision, message_connector::send_message,
    tool_call_connector::execute_tool,
};

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

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
                UserState::Idle(last_conversation),
                UserAction::NewMessage {
                    msg,
                    start_conversation: true,
                },
            ) => {
                let mut external = Vec::<UserExternalOperation>::new();

                let summary = match last_conversation {
                    Some((recent_conversation, _)) => recent_conversation.summary.clone(),
                    None => "".to_string(),
                };

                external.push(Box::pin(get_llm_decision(
                    env.clone(),
                    msg.clone(),
                    summary,
                    Vec::new(), // No previous tool calls for new messages
                )));

                let user = User {
                    state: UserState::AwaitingLLMDecision {
                        is_timeout: false,
                        previous_tool_calls: Vec::new(),
                    },
                    last_transition: Utc::now(),
                };

                println!("Id: {0} {1:?}", user_id.1, user.state);

                Ok((user, external))
            }
            (
                UserState::AwaitingLLMDecision {
                    is_timeout,
                    previous_tool_calls,
                },
                UserAction::LLMDecisionResult(res),
            ) => match &**res {
                Ok((summary, outcome)) => {
                    // Extract message to send from outcome
                    let message_to_send = match outcome {
                        MessageOutcome::Final { response } => Some(response.clone()),
                        MessageOutcome::IntermediateToolCall {
                            maybe_intermediate_response,
                            ..
                        } => maybe_intermediate_response.clone(),
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
                                message.clone(),
                            )));

                            Ok((
                                User {
                                    state: UserState::SendingMessage {
                                        is_timeout,
                                        outcome: outcome.clone(),
                                        recent_conversation: RecentConversation {
                                            summary: summary.clone(),
                                        },
                                        previous_tool_calls: previous_tool_calls.clone(),
                                    },
                                    last_transition: Utc::now(),
                                },
                                external,
                            ))
                        }
                        None => {
                            // Silent tool call - go directly to RunningTool
                            match outcome {
                                MessageOutcome::IntermediateToolCall { tool_call, .. } => {
                                    let mut external = Vec::<UserExternalOperation>::new();
                                    external.push(Box::pin(execute_tool(
                                        env.clone(),
                                        tool_call.clone(),
                                    )));

                                    Ok((
                                        User {
                                            state: UserState::RunningTool {
                                                is_timeout,
                                                recent_conversation: RecentConversation {
                                                    summary: summary.clone(),
                                                },
                                                previous_tool_calls: previous_tool_calls.clone(),
                                            },
                                            last_transition: Utc::now(),
                                        },
                                        external,
                                    ))
                                }
                                MessageOutcome::Final { .. } => {
                                    // This shouldn't happen (Final always has a message)
                                    // But handle it gracefully
                                    Ok((
                                        User {
                                            state: UserState::Idle(if is_timeout {
                                                None
                                            } else {
                                                Some((
                                                    RecentConversation {
                                                        summary: summary.clone(),
                                                    },
                                                    Utc::now(),
                                                ))
                                            }),
                                            last_transition: Utc::now(),
                                        },
                                        Vec::new(),
                                    ))
                                }
                            }
                        }
                    }
                }
                Err(_) => Ok((
                    User {
                        state: UserState::Idle(None),
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
                    previous_tool_calls,
                },
                UserAction::MessageSent(_res),
            ) => {
                // Ignore errors from message sending - continue with normal flow regardless
                // Message sent (or failed, but we don't care) - check outcome to determine next state
                match outcome {
                    MessageOutcome::Final { .. } => {
                        // Final response sent - transition to Idle
                        Ok((
                            User {
                                state: UserState::Idle(if is_timeout {
                                    None
                                } else {
                                    Some((recent_conversation.clone(), Utc::now()))
                                }),
                                last_transition: Utc::now(),
                            },
                            Vec::new(),
                        ))
                    }
                    MessageOutcome::IntermediateToolCall { tool_call, .. } => {
                        // Intermediate message sent - now execute the tool
                        let mut external = Vec::<UserExternalOperation>::new();
                        external.push(Box::pin(execute_tool(env.clone(), tool_call.clone())));

                        Ok((
                            User {
                                state: UserState::RunningTool {
                                    is_timeout,
                                    recent_conversation: recent_conversation.clone(),
                                    previous_tool_calls: previous_tool_calls.clone(),
                                },
                                last_transition: Utc::now(),
                            },
                            external,
                        ))
                    }
                }
            }
            (
                UserState::RunningTool {
                    recent_conversation,
                    previous_tool_calls,
                    is_timeout,
                    ..
                },
                UserAction::ToolResult(res),
            ) => {
                match &**res {
                    Ok(tool_result) => {
                        // Add tool result to previous tool calls
                        let mut updated_tool_calls = previous_tool_calls.clone();
                        updated_tool_calls.push(tool_result.clone());

                        // Tool execution complete - get next LLM decision with tool results
                        let mut external = Vec::<UserExternalOperation>::new();
                        external.push(Box::pin(get_llm_decision(
                            env.clone(),
                            "Continue conversation".to_string(), // Dummy message for tool call continuation
                            recent_conversation.summary.clone(),
                            updated_tool_calls.clone(),
                        )));

                        Ok((
                            User {
                                state: UserState::AwaitingLLMDecision {
                                    is_timeout,
                                    previous_tool_calls: updated_tool_calls,
                                },
                                last_transition: Utc::now(),
                            },
                            external,
                        ))
                    }
                    Err(_) => Ok((
                        User {
                            state: UserState::Idle(None),
                            last_transition: Utc::now(),
                        },
                        Vec::new(),
                    )),
                }
            }
            (UserState::Idle(Some((recent_conversation, _))), UserAction::Timeout) => {
                println!("Timed Out");

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(get_llm_decision(
                    env.clone(),
                    "User said goodbye, RESPOND WITH GOODBYE BUT MENTION RELEVANT THINGS ABOUT THE CONVERSATION".to_string(),
                    recent_conversation.summary.clone(),
                    Vec::new(), // No previous tool calls for timeout
                )));

                Ok((
                    User {
                        state: UserState::AwaitingLLMDecision {
                            is_timeout: true,
                            previous_tool_calls: Vec::new(),
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
        UserState::Idle(Some((_, last_activity))) => schedules.push(Scheduled {
            at: last_activity + ChronoDuration::milliseconds(300_000),
            action: UserAction::Timeout,
        }),
        UserState::AwaitingLLMDecision { .. }
        | UserState::SendingMessage { .. }
        | UserState::RunningTool { .. } => schedules.push(Scheduled {
            at: user.last_transition + ChronoDuration::milliseconds(120_000),
            action: UserAction::ForceReset,
        }),
        _ => {}
    }

    schedules
}

pub static USER_LIFE_CYCLE: Lazy<lib_hive::LifeCycleHandle<UserId, UserAction>> =
    Lazy::new(|| new_life_cycle(ENV.clone(), Transition(user_transition), Schedule(schedule)));
