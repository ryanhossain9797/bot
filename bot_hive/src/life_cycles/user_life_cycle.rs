use std::{future::Future, pin::Pin, sync::Arc};

use crate::{
    models::user::{MessageOutcome, RecentConversation, ToolCall, User, UserAction, UserId, UserState},
    Env, ENV,
};
use chrono::{Duration as ChronoDuration, Utc};
use lib_hive::{
    new_life_cycle, ExternalOperation, Schedule, Scheduled, Transition, TransitionResult,
};
use once_cell::sync::Lazy;

use crate::connectors::user_connector::handle_bot_message;

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

                external.push(Box::pin(handle_bot_message(
                    env.clone(),
                    user_id.clone(),
                    msg.clone(),
                    summary,
                    Vec::new(), // No previous tool calls for new messages
                )));

                let user = User {
                    state: UserState::sending_message(false),
                };

                println!("Id: {0} {1:?}", user_id.1, user.state);

                Ok((user, external))
            }
            (UserState::SendingMessage { is_timeout, previous_tool_calls }, UserAction::SendResult(res)) => 
            match &**res {
                Ok((summary, outcome)) => {
                    match outcome {
                        MessageOutcome::Final { .. } => {
                            // Final response - transition to Idle
                            Ok((
                                User {
                                    state: UserState::Idle(if is_timeout { None } else { Some((RecentConversation { summary: summary.clone() }, Utc::now())) }),
                                },
                                Vec::new(),
                            ))
                        }
                        MessageOutcome::IntermediateToolCall { tool_call, .. } => {
                            // Execute tool (fake for now) and loop back
                            let tool_result = execute_tool_fake(tool_call);
                            let mut updated_tool_calls = previous_tool_calls.clone();
                            updated_tool_calls.push(tool_result);

                            // Continue the loop - call handle_bot_message again with updated tool calls
                            let mut external = Vec::<UserExternalOperation>::new();
                            external.push(Box::pin(handle_bot_message(
                                env.clone(),
                                user_id.clone(),
                                "Continue conversation".to_string(), // Dummy message for tool call continuation
                                summary.clone(),
                                updated_tool_calls.clone(),
                            )));

                            Ok((
                                User {
                                    state: UserState::SendingMessage {
                                        is_timeout,
                                        previous_tool_calls: updated_tool_calls,
                                    },
                                },
                                external,
                            ))
                        }
                    }
                }
                Err(_) => Ok((
                    User {
                        state: UserState::Idle(None),
                    },
                    Vec::new(),
                )),
            },
            (UserState::Idle(Some((recent_conversation, _))), UserAction::Timeout) => {
                println!("Timed Out");

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(handle_bot_message(
                    env.clone(),
                    user_id.clone(),
                    "User said goodbye, RESPOND WITH GOODBYE BUT MENTION RELEVANT THINGS ABOUT THE CONVERSATION".to_string(),
                    recent_conversation.summary.clone(),
                    Vec::new(), // No previous tool calls for timeout
                )));

                Ok((
                    User {
                        state: UserState::sending_message(true),
                    },
                    external,
                ))
            }
            _ => Err(anyhow::anyhow!("Invalid state or action")),
        }
    })
}

pub fn schedule(user: &User) -> Vec<Scheduled<UserAction>> {
    match user.state {
        UserState::Idle(Some((_, last_activity))) => {
            vec![Scheduled {
                at: last_activity + ChronoDuration::milliseconds(300_000),
                action: UserAction::Timeout,
            }]
        }
        _ => Vec::new(),
    }
}

fn execute_tool_fake(tool_call: &ToolCall) -> String {
    match tool_call {
        ToolCall::DeviceControl { device, property, value } => {
            format!("Tool call set {} {} {} | Result: Success", device, property, value)
        }
    }
}

pub static USER_LIFE_CYCLE: Lazy<lib_hive::LifeCycleHandle<UserId, UserAction>> =
    Lazy::new(|| new_life_cycle(ENV.clone(), Transition(user_transition), Schedule(schedule)));
