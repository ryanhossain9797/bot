use crate::externals::{
    llama_cpp_external::get_llm_decision,
    message_external::{send_message, OutboundMessage},
    tool_call_external::execute_tool,
};
use crate::types::conversation::{
    latest_file_hash, ToolResult, ToolResultData, ToolType, MAX_TOOL_ROUNDS,
};
use crate::state_machines::memory_manager_state_machine::MemoryManagerMachine;
use crate::types::media::Attachment;
use crate::types::memory::{MemoryManagerAction, MemoryManagerConstructor};
use crate::{
    types::conversation::{
        CompactionOutput, Conversation, ConversationAction, ConversationConstructor,
        ConversationId, ConversationMessage, ConversationState, HistoryEntry, HistoryEntryKind,
        InterruptionReason, LLMInput, PostSend, RecentConversation,
    },
    Env,
};
use chrono::{Duration as ChronoDuration, Utc};
use re_framework::{Effects, Scheduled, StateMachine};
use std::collections::HashMap;
use std::sync::Arc;

type ConversationTransitionResult = anyhow::Result<Conversation>;
type ConversationEffects = Effects<ConversationMachine>;

const REDACT_HISTORY_IMAGES: bool = false;

const LLM_TIMEOUT_MS: i64 = 300_000;
const SEND_TIMEOUT_MS: i64 = 60_000;

const COMPACT_WATERMARK: usize = 24;
const KEEP_RECENT: usize = 12;

pub struct ConversationMachine;

fn compact_state(
    conversation_id: &ConversationId,
    state: ConversationState,
    output: &CompactionOutput,
) -> ConversationState {
    match state {
        ConversationState::Idle {
            recent_conversation,
        } => ConversationState::Idle {
            recent_conversation: compact_recent(conversation_id, recent_conversation, output),
        },
        ConversationState::SendingMessage {
            recent_conversation,
            post_send,
        } => ConversationState::SendingMessage {
            recent_conversation: compact_recent(conversation_id, recent_conversation, output),
            post_send,
        },
        ConversationState::RunningTools {
            recent_conversation,
            tool_rounds,
            pending_tools,
            completed_tools,
        } => ConversationState::RunningTools {
            recent_conversation: compact_recent(conversation_id, recent_conversation, output),
            tool_rounds,
            pending_tools,
            completed_tools,
        },
        ConversationState::AwaitingLLMDecision {
            history,
            current_input,
            tool_rounds,
        } => ConversationState::AwaitingLLMDecision {
            history: compact_history(conversation_id, history, output),
            current_input,
            tool_rounds,
        },
    }
}

fn compact_recent(
    conversation_id: &ConversationId,
    recent_conversation: RecentConversation,
    output: &CompactionOutput,
) -> RecentConversation {
    let RecentConversation { thoughts, history } = recent_conversation;
    let compacted = compact_history(conversation_id, history.into(), output);
    RecentConversation::new(thoughts, compacted)
}

fn compact_history(
    conversation_id: &ConversationId,
    history: Vec<HistoryEntry>,
    output: &CompactionOutput,
) -> Vec<HistoryEntry> {
    let Some(boundary) = history.iter().position(|entry| entry.id == output.through) else {
        println!("[compact] {conversation_id} boundary already compacted away — no-op");
        return history;
    };

    let mut history = history;
    let tail = history.split_off(boundary + 1);
    let kept = tail.len();
    let dropped = boundary + 1;
    let mut compacted = Vec::with_capacity(kept + 1);
    compacted.push(HistoryEntry::summary(output.summary.clone()));
    compacted.extend(tail);
    println!(
        "[compact] {conversation_id} applied summary — replaced {dropped} entries, kept {kept}"
    );
    compacted
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
    compaction_in_flight: bool,
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
            compaction_in_flight,
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
        compaction_in_flight,
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
    compaction_in_flight: bool,
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
            compaction_in_flight,
        }),
        PostSend::CallTools {
            tool_rounds,
            tool_calls,
        } => {
            let history = recent_conversation.history();
            let mut pending_tools = HashMap::new();
            for tool_call in tool_calls {
                let expected_file_hash = match &tool_call.tool_type {
                    ToolType::EditFile { path, .. } => {
                        latest_file_hash(&history, path).map(str::to_string)
                    }
                    _ => None,
                };
                effects.enqueue_external(execute_tool(
                    conversation_id.to_string(),
                    tool_call.clone(),
                    expected_file_hash,
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
                compaction_in_flight,
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
                compaction_in_flight,
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
        (
            user_state,
            ConversationAction::NewMessage {
                msg,
                user_id,
                name,
                attachments,
            },
        ) => {
            let mut pending = conversation.pending;
            let queued = !matches!(&user_state, ConversationState::Idle { .. });
            pending.push(ConversationMessage {
                text: msg.clone(),
                queued,
                user_id: user_id.clone(),
                name: name.clone(),
                attachments: attachments.iter().map(Attachment::downscaled).collect(),
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
                updated_history.push(HistoryEntry::input(recorded_input));
                updated_history.push(HistoryEntry::output(response.clone()));

                let updated_conversation =
                    RecentConversation::new(response.thoughts.clone(), updated_history);

                let announce_tools: Vec<String> = if env.announce_tool_use {
                    response
                        .tool_calls
                        .iter()
                        .filter_map(|tc| tc.tool_type.announcement().map(str::to_string))
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
                        message: response.message().map(str::to_string),
                        tool_names: announce_tools,
                    },
                    post_send,
                    updated_conversation,
                    conversation.pending,
                    conversation.is_group,
                    conversation.bot_identity.clone(),
                    conversation.compaction_in_flight,
                    effects,
                )
            }
            Err(reason) => {
                let mut history = history;
                history.push(HistoryEntry::interrupted(reason.clone()));
                Ok(Conversation {
                    state: ConversationState::Idle {
                        recent_conversation: RecentConversation::new(String::new(), history),
                    },
                    last_transition: Utc::now(),
                    ..conversation
                })
            }
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
            conversation.compaction_in_flight,
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

                let results = completed_tools
                    .into_iter()
                    .map(|(call, data)| ToolResult { call, data })
                    .collect();

                let mut pending = conversation.pending;
                let followup = take_pending(&mut pending);

                send_then(
                    env,
                    conversation_id,
                    OutboundMessage {
                        message: None,
                        tool_names: Vec::new(),
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
                    conversation.compaction_in_flight,
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
                    ..conversation
                })
            }
        }
        (current_state, ConversationAction::CompactionResult(result)) => {
            let state = match result {
                Ok(output) => compact_state(conversation_id, current_state, output),
                Err(reason) => {
                    println!("[compact] {conversation_id} compaction did not complete: {reason:?}");
                    current_state
                }
            };
            Ok(Conversation {
                state,
                compaction_in_flight: false,
                ..conversation
            })
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
        let attachments = drained.iter().flat_map(|m| m.attachments.clone()).collect();
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
            attachments,
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
        maybe_fire_compaction(env, conversation_id, &mut conversation, &recent_conversation, effects);
        return Ok(conversation);
    };

    let is_group = conversation.is_group;
    let bot_identity = conversation.bot_identity.clone();
    let compaction_in_flight = conversation.compaction_in_flight;

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
        compaction_in_flight,
    })
}

fn maybe_fire_compaction(
    env: &Arc<Env>,
    conversation_id: &ConversationId,
    conversation: &mut Conversation,
    recent_conversation: &RecentConversation,
    effects: &mut ConversationEffects,
) {
    if env.utility.is_none() || conversation.compaction_in_flight {
        return;
    }

    let history = recent_conversation.history();
    let compactable = history
        .iter()
        .filter(|entry| !matches!(entry.kind, HistoryEntryKind::Summary(_)))
        .count();
    if compactable < COMPACT_WATERMARK {
        return;
    }

    let prefix_len = history.len().saturating_sub(KEEP_RECENT);
    if prefix_len == 0 {
        return;
    }

    let prefix = history[..prefix_len].to_vec();
    effects.enqueue_act_maybe_construct::<MemoryManagerMachine>(
        MemoryManagerConstructor {
            id: conversation_id.clone(),
        },
        MemoryManagerAction::Compact { history: prefix },
    );
    conversation.compaction_in_flight = true;
    println!("[compact] {conversation_id} firing compaction of {prefix_len} entries");
}

fn conversation_schedule(conversation: &Conversation) -> Option<Scheduled<ConversationAction>> {
    match &conversation.state {
        ConversationState::Idle { .. } => None,
        ConversationState::AwaitingLLMDecision { .. } => Some(Scheduled {
            at: conversation.last_transition + ChronoDuration::milliseconds(LLM_TIMEOUT_MS),
            action: ConversationAction::LLMDecisionResult(Err(InterruptionReason::TimedOut)),
        }),
        ConversationState::SendingMessage { .. } => Some(Scheduled {
            at: conversation.last_transition + ChronoDuration::milliseconds(SEND_TIMEOUT_MS),
            action: ConversationAction::MessageSent(Err("timed out".to_string())),
        }),
        ConversationState::RunningTools { pending_tools, .. } => pending_tools
            .iter()
            .map(|(id, call)| {
                (id.clone(), conversation.last_transition + call.tool_type.rescue_timeout())
            })
            .min_by_key(|(_, at)| *at)
            .map(|(id, at)| Scheduled {
                at,
                action: ConversationAction::ToolResult {
                    id,
                    result: Err("timed out".to_string()),
                },
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
        compaction_in_flight: false,
    }
}

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

    fn name() -> &'static str {
        "ConversationMachine"
    }
}
