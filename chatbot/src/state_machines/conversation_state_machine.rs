use crate::externals::{
    llama_cpp_external::get_llm_decision,
    message_external::{send_message, Attachment, OutboundMessage},
    tool_call_external::execute_tool,
};
use crate::types::conversation::{ToolResult, ToolResultData, MAX_TOOL_ROUNDS};
use crate::{
    types::conversation::{
        Conversation, ConversationAction, ConversationConstructor, ConversationId,
        ConversationMessage, ConversationState, HistoryEntry, LLMInput, PostSend,
        RecentConversation,
    },
    Env,
};
use chrono::{Duration as ChronoDuration, Utc};
use re_framework::{Effects, Scheduled, StateMachine, StateMachineHandle};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

type ConversationTransitionResult = anyhow::Result<Conversation>;
type ConversationEffects = Effects<ConversationMachine>;

const REDACT_HISTORY_IMAGES: bool = false;

static CONVERSATION: OnceLock<StateMachineHandle<ConversationMachine>> = OnceLock::new();

pub struct ConversationMachine;

impl StateMachine for ConversationMachine {
    type State = Conversation;
    type Id = ConversationId;
    type Action = ConversationAction;
    type Construction = ConversationConstructor;
    type Env = crate::Env;

    fn construct(
        constructor: ConversationConstructor,
        _effects: &mut ConversationEffects,
    ) -> Conversation {
        construct_conversation(constructor)
    }

    fn transition(
        state: &Conversation,
        id: &ConversationId,
        env: &Arc<Env>,
        action: &ConversationAction,
        effects: &mut ConversationEffects,
    ) -> ConversationTransitionResult {
        conversation_transition(env, id, state.clone(), action, effects)
    }

    fn schedule(state: &Conversation) -> Option<Scheduled<ConversationAction>> {
        conversation_schedule(state)
    }

    fn handle() -> &'static StateMachineHandle<ConversationMachine> {
        CONVERSATION
            .get()
            .expect("ConversationMachine not initialized")
    }
}

pub fn init_conversation_state_machine(env: Env) {
    let handle = StateMachineHandle::<ConversationMachine>::new(env);
    CONVERSATION
        .set(handle)
        .ok()
        .expect("ConversationMachine initialized once");
}

fn state_label(state: &ConversationState) -> &'static str {
    match state {
        ConversationState::Idle { .. } => "Idle",
        ConversationState::AwaitingLLMDecision { .. } => "AwaitingLLMDecision",
        ConversationState::SendingMessage { .. } => "SendingMessage",
        ConversationState::RunningTools { .. } => "RunningTools",
    }
}

fn send_then(
    env: &Arc<Env>,
    conversation_id: &ConversationId,
    outbound: OutboundMessage,
    post_send: PostSend,
    recent_conversation: RecentConversation,
    pending: Vec<ConversationMessage>,
    is_group: bool,
    bot_identity: String,
    effects: &mut ConversationEffects,
) -> ConversationTransitionResult {
    if outbound.is_empty() {
        return apply_post_send(
            env,
            conversation_id,
            post_send,
            recent_conversation,
            pending,
            is_group,
            bot_identity,
            effects,
        );
    }

    effects.enqueue_external(send_message(
        Arc::clone(env),
        conversation_id.clone(),
        outbound,
    ));

    Ok(Conversation {
        state: ConversationState::SendingMessage {
            recent_conversation,
            post_send,
        },
        last_transition: Utc::now(),
        pending,
        is_group,
        bot_identity,
    })
}

fn apply_post_send(
    env: &Arc<Env>,
    conversation_id: &ConversationId,
    post_send: PostSend,
    recent_conversation: RecentConversation,
    pending: Vec<ConversationMessage>,
    is_group: bool,
    bot_identity: String,
    effects: &mut ConversationEffects,
) -> ConversationTransitionResult {
    match post_send {
        PostSend::Nothing => Ok(Conversation {
            state: ConversationState::Idle {
                recent_conversation,
            },
            last_transition: Utc::now(),
            pending,
            is_group,
            bot_identity,
        }),
        PostSend::CallTools {
            tool_rounds,
            tool_calls,
        } => {
            let mut pending_tools = HashMap::new();
            for tool_call in tool_calls {
                effects.enqueue_external(execute_tool(conversation_id.to_string(), tool_call.clone()));
                pending_tools.insert(tool_call.id.clone(), tool_call);
            }

            Ok(Conversation {
                state: ConversationState::RunningTools {
                    recent_conversation,
                    tool_rounds: tool_rounds + 1,
                    pending_tools,
                    completed_tools: Vec::new(),
                },
                last_transition: Utc::now(),
                pending,
                is_group,
                bot_identity,
            })
        }
        PostSend::SendToolResponse {
            tool_rounds,
            results,
            followup,
        } => {
            let current_input = LLMInput::ToolResults(results, followup);
            let history = recent_conversation.history();
            effects.enqueue_external(get_llm_decision(
                Arc::clone(env),
                current_input.clone(),
                Some(recent_conversation),
                tool_rounds,
                MAX_TOOL_ROUNDS,
                is_group,
                bot_identity.clone(),
                conversation_id.0.clone(),
            ));

            Ok(Conversation {
                state: ConversationState::AwaitingLLMDecision {
                    history,
                    current_input,
                    tool_rounds,
                },
                last_transition: Utc::now(),
                pending,
                is_group,
                bot_identity,
            })
        }
    }
}

fn conversation_transition(
    env: &Arc<Env>,
    conversation_id: &ConversationId,
    conversation: Conversation,
    action: &ConversationAction,
    effects: &mut ConversationEffects,
) -> ConversationTransitionResult {
    let from = state_label(&conversation.state);
    let state = match (conversation.state, action) {
        (_, ConversationAction::ForceReset) => Ok(Conversation {
            pending: Vec::new(),
            state: ConversationState::default(),
            last_transition: Utc::now(),
            ..conversation
        }),
        (
            user_state,
            ConversationAction::NewMessage {
                msg,
                user_id,
                name,
                images,
            },
        ) => {
            let mut pending = conversation.pending;
            let queued = !matches!(&user_state, ConversationState::Idle { .. });
            pending.push(ConversationMessage {
                text: msg.clone(),
                queued,
                user_id: user_id.clone(),
                name: name.clone(),
                images: images.iter().map(|image| image.downscaled()).collect(),
            });

            Ok(Conversation {
                pending,
                state: user_state,
                ..conversation
            })
        }
        (
            ConversationState::AwaitingLLMDecision {
                history,
                current_input,
                tool_rounds,
            },
            ConversationAction::LLMDecisionResult(res),
        ) => match res {
            Ok(response) => {
                let recorded_input = if REDACT_HISTORY_IMAGES {
                    current_input.redacted()
                } else {
                    current_input
                };
                let mut updated_history = history;
                updated_history.push(HistoryEntry::Input(recorded_input));
                updated_history.push(HistoryEntry::Output(response.clone()));

                let updated_conversation =
                    RecentConversation::new(response.thoughts.clone(), updated_history);

                let announce_tools: Vec<String> = if env.announce_tool_use {
                    response
                        .tool_calls
                        .iter()
                        .map(|tc| tc.tool_type.wire_name().to_string())
                        .collect()
                } else {
                    Vec::new()
                };

                let post_send = if response.tool_calls.is_empty() {
                    PostSend::Nothing
                } else {
                    PostSend::CallTools {
                        tool_rounds,
                        tool_calls: response.tool_calls.clone(),
                    }
                };

                send_then(
                    env,
                    conversation_id,
                    OutboundMessage {
                        message: response.message.clone(),
                        tool_names: announce_tools,
                        attachments: Vec::new(),
                    },
                    post_send,
                    updated_conversation,
                    conversation.pending,
                    conversation.is_group,
                    conversation.bot_identity.clone(),
                    effects,
                )
            }
            Err(_) => Ok(Conversation {
                state: ConversationState::Idle {
                    recent_conversation: RecentConversation::new(String::new(), history),
                },
                last_transition: Utc::now(),
                ..conversation
            }),
        },
        (
            ConversationState::SendingMessage {
                recent_conversation,
                post_send,
            },
            ConversationAction::MessageSent(_res),
        ) => apply_post_send(
            env,
            conversation_id,
            post_send,
            recent_conversation,
            conversation.pending,
            conversation.is_group,
            conversation.bot_identity.clone(),
            effects,
        ),
        (
            ConversationState::RunningTools {
                recent_conversation,
                tool_rounds,
                mut pending_tools,
                mut completed_tools,
            },
            ConversationAction::ToolResult { id, result },
        ) => {
            if let Some(tool_call) = pending_tools.remove(id) {
                let data = match result {
                    Ok(data) => data.clone(),
                    Err(error_msg) => {
                        let msg = format!("Tool execution failed: {error_msg}");
                        ToolResultData::text(msg.clone(), msg)
                    }
                };
                completed_tools.push((tool_call, data));
            } else {
                eprintln!("[warn] tool result for unknown id {id}; ignoring");
            }

            if pending_tools.is_empty() {
                completed_tools.sort_by(|(a, _), (b, _)| a.id.cmp(&b.id));

                let attachments: Vec<Attachment> = completed_tools
                    .iter()
                    .filter_map(|(_, data)| data.image_for_user.clone())
                    .map(Attachment::Image)
                    .collect();

                let results = completed_tools
                    .into_iter()
                    .map(|(tool_call, data)| ToolResult {
                        id: tool_call.id,
                        data,
                    })
                    .collect();

                let mut pending = conversation.pending;
                let followup = take_pending(&mut pending);

                send_then(
                    env,
                    conversation_id,
                    OutboundMessage {
                        message: None,
                        tool_names: Vec::new(),
                        attachments,
                    },
                    PostSend::SendToolResponse {
                        tool_rounds,
                        results,
                        followup,
                    },
                    recent_conversation,
                    pending,
                    conversation.is_group,
                    conversation.bot_identity.clone(),
                    effects,
                )
            } else {
                Ok(Conversation {
                    state: ConversationState::RunningTools {
                        recent_conversation,
                        tool_rounds,
                        pending_tools,
                        completed_tools,
                    },
                    last_transition: Utc::now(),
                    ..conversation
                })
            }
        }
        _ => Err(anyhow::anyhow!(
            "no transition for {action:?} in state {from}"
        )),
    };

    post_transition(env, conversation_id, state, effects)
}

fn take_pending(pending: &mut Vec<ConversationMessage>) -> Option<ConversationMessage> {
    (!pending.is_empty()).then(|| {
        let drained = std::mem::take(pending);
        let queued = drained.iter().any(|m| m.queued);
        let user_id = drained
            .last()
            .map(|m| m.user_id.clone())
            .unwrap_or_default();
        let name = drained.last().map(|m| m.name.clone()).unwrap_or_default();
        let images = drained.iter().flat_map(|m| m.images.clone()).collect();
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
            images,
        }
    })
}

fn post_transition(
    env: &Arc<Env>,
    conversation_id: &ConversationId,
    result: ConversationTransitionResult,
    effects: &mut ConversationEffects,
) -> ConversationTransitionResult {
    let mut conversation = result?;

    let recent_conversation = match &conversation.state {
        ConversationState::Idle {
            recent_conversation,
        } => recent_conversation.clone(),
        _ => return Ok(conversation),
    };

    let Some(msg) = take_pending(&mut conversation.pending) else {
        return Ok(conversation);
    };

    let is_group = conversation.is_group;
    let bot_identity = conversation.bot_identity.clone();

    let history = recent_conversation.history();

    let current_input = LLMInput::ConversationMessage(msg);

    effects.enqueue_external(get_llm_decision(
        Arc::clone(env),
        current_input.clone(),
        Some(recent_conversation),
        0,
        MAX_TOOL_ROUNDS,
        is_group,
        bot_identity.clone(),
        conversation_id.0.clone(),
    ));

    Ok(Conversation {
        state: ConversationState::AwaitingLLMDecision {
            history,
            current_input,
            tool_rounds: 0,
        },
        last_transition: Utc::now(),
        pending: Vec::new(),
        is_group,
        bot_identity,
    })
}

fn conversation_schedule(conversation: &Conversation) -> Option<Scheduled<ConversationAction>> {
    match conversation.state {
        ConversationState::Idle { .. } => None,
        ConversationState::AwaitingLLMDecision { .. }
        | ConversationState::SendingMessage { .. }
        | ConversationState::RunningTools { .. } => Some(Scheduled {
            at: conversation.last_transition + ChronoDuration::milliseconds(600_000),
            action: ConversationAction::ForceReset,
        }),
    }
}

fn construct_conversation(constructor: ConversationConstructor) -> Conversation {
    Conversation {
        pending: Vec::new(),
        state: ConversationState::default(),
        last_transition: Utc::now(),
        is_group: constructor.is_group,
        bot_identity: constructor.bot_identity,
    }
}
