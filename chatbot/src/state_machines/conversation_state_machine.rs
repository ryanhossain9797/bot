use crate::externals::long_term_memory_external::commit_to_memory;
use crate::externals::{
    llama_cpp_external::get_llm_decision, message_external::send_message,
    tool_call_external::execute_tool,
};
use crate::types::conversation::{LLMResponse, ToolResult, ToolResultData, MAX_TOOL_ROUNDS};
use crate::{
    types::conversation::{
        Conversation, ConversationAction, ConversationConstructor, ConversationId,
        ConversationMessage, ConversationState, HistoryEntry, LLMInput, RecentConversation,
    },
    Env,
};
use chrono::{Duration as ChronoDuration, Utc};
use re_framework::{Effects, Scheduled, StateMachine, StateMachineHandle};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

type ConversationTransitionResult = anyhow::Result<Conversation>;
type ConversationEffects = Effects<ConversationMachine>;

/// When true, images are dehydrated (dropped to a note) before entering history. When false
/// they stay hydrated and are re-fed to the model every turn — experimental, grows context
/// and VRAM with each turn that carries an image.
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
        ConversationState::CommitingToMemory { .. } => "CommitingToMemory",
        ConversationState::AwaitingLLMDecision { .. } => "AwaitingLLMDecision",
        ConversationState::SendingMessage { .. } => "SendingMessage",
        ConversationState::RunningTools { .. } => "RunningTools",
    }
}

fn handle_outcome(
    env: &Arc<Env>,
    conversation_id: &ConversationId,
    response: LLMResponse,
    recent_conversation: RecentConversation,
    pending: Vec<ConversationMessage>,
    tool_rounds: usize,
    is_group: bool,
    bot_identity: String,
    effects: &mut ConversationEffects,
) -> ConversationTransitionResult {
    if response.tool_calls.is_empty() {
        Ok(Conversation {
            state: ConversationState::Idle {
                recent_conversation: Some((recent_conversation, Utc::now())),
            },
            last_transition: Utc::now(),
            pending,
            is_group,
            bot_identity,
        })
    } else {
        let mut pending_tools = HashMap::new();
        for tool_call in response.tool_calls {
            effects.enqueue_external(execute_tool(
                Arc::clone(&env),
                tool_call.clone(),
                conversation_id.to_string(),
                recent_conversation.history.clone(),
            ));
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
        (user_state, ConversationAction::NewMessage { msg, user_id, name, images }) => {
            let mut pending = conversation.pending;
            let queued = !matches!(&user_state, ConversationState::Idle { .. });
            pending.push(ConversationMessage {
                text: msg.clone(),
                queued,
                user_id: user_id.clone(),
                name: name.clone(),
                images: images.clone(),
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

                let updated_conversation = RecentConversation {
                    thoughts: response.thoughts.clone(),
                    history: updated_history,
                };

                let message_to_send = response.message.clone().or_else(|| {
                    (!response.tool_calls.is_empty() && env.announce_tool_use).then(|| {
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
                        effects.enqueue_external(send_message(
                            Arc::clone(&env),
                            conversation_id.clone(),
                            message,
                        ));

                        Ok(Conversation {
                            state: ConversationState::SendingMessage {
                                outcome: response.clone(),
                                recent_conversation: updated_conversation,
                                tool_rounds,
                            },
                            last_transition: Utc::now(),
                            ..conversation
                        })
                    }
                    None => handle_outcome(
                        env,
                        conversation_id,
                        response.clone(),
                        updated_conversation,
                        conversation.pending,
                        tool_rounds,
                        conversation.is_group,
                        conversation.bot_identity.clone(),
                        effects,
                    ),
                }
            }
            Err(_) => Ok(Conversation {
                state: ConversationState::Idle {
                    recent_conversation: None,
                },
                last_transition: Utc::now(),
                ..conversation
            }),
        },
        (
            ConversationState::SendingMessage {
                outcome,
                recent_conversation,
                tool_rounds,
            },
            ConversationAction::MessageSent(_res),
        ) => handle_outcome(
            env,
            conversation_id,
            outcome,
            recent_conversation,
            conversation.pending,
            tool_rounds,
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
                completed_tools.sort_by(|(a, _), (b, _)| a.id.cmp(&b.id));
                let results = completed_tools
                    .into_iter()
                    .map(|(tool_call, data)| ToolResult {
                        id: tool_call.id,
                        data,
                    })
                    .collect();
                let mut pending = conversation.pending;
                let current_input = LLMInput::ToolResults(results, take_pending(&mut pending));
                let is_group = conversation.is_group;
                let bot_identity = conversation.bot_identity.clone();

                let history = recent_conversation.history();
                effects.enqueue_external(get_llm_decision(
                    Arc::clone(&env),
                    current_input.clone(),
                    Some(recent_conversation),
                    tool_rounds,
                    MAX_TOOL_ROUNDS,
                    is_group,
                    bot_identity.clone(),
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
        (
            ConversationState::Idle {
                recent_conversation: Some((recent_conversation, _)),
            },
            ConversationAction::Timeout,
        ) => {
            println!("Timed Out");

            effects.enqueue_external(commit_to_memory(
                Arc::clone(&env),
                conversation_id.to_string(),
                recent_conversation.history.clone(),
            ));

            Ok(Conversation {
                state: ConversationState::CommitingToMemory {
                    recent_conversation,
                },
                last_transition: Utc::now(),
                ..conversation
            })
        }
        (ConversationState::CommitingToMemory { .. }, ConversationAction::CommitResult(_)) => {
            println!("Commited to Memory");

            Ok(Conversation {
                state: ConversationState::Idle {
                    recent_conversation: None,
                },
                last_transition: Utc::now(),
                ..conversation
            })
        }
        _ => Err(anyhow::anyhow!("no transition for {action:?} in state {from}")),
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
    _conversation_id: &ConversationId,
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

    let history = recent_conversation
        .as_ref()
        .map(|(rc, _)| rc.history())
        .unwrap_or_else(Vec::new);

    let current_input = LLMInput::ConversationMessage(msg);

    effects.enqueue_external(get_llm_decision(
        Arc::clone(&env),
        current_input.clone(),
        recent_conversation.map(|(rc, _)| rc),
        0,
        MAX_TOOL_ROUNDS,
        is_group,
        bot_identity.clone(),
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
        ConversationState::Idle {
            recent_conversation: Some((_, last_activity)),
        } => Some(Scheduled {
            at: last_activity + ChronoDuration::milliseconds(900_000),
            action: ConversationAction::Timeout,
        }),
        ConversationState::AwaitingLLMDecision { .. }
        | ConversationState::SendingMessage { .. }
        | ConversationState::CommitingToMemory { .. }
        | ConversationState::RunningTools { .. } => Some(Scheduled {
            at: conversation.last_transition + ChronoDuration::milliseconds(600_000),
            action: ConversationAction::ForceReset,
        }),
        _ => None,
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
