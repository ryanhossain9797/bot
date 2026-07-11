//! Minimal replica of the chatbot's ConversationMachine: same four-phase shape
//! (Idle / AwaitingDecision / RunningTool / SendingReply), same decision loop
//! (tool results go back through the brain), deterministic externals instead of an LLM.

use crate::externals::{decide, execute_tool, send_reply, BrainInput};
use crate::stats::{StatsAction, StatsId, StatsMachine};
use chrono::{DateTime, Duration, Utc};
use re_framework::{Effects, EntityId, Identified, Scheduled, StateMachine, StateMachineHandle};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

const IDLE_RESET_SECS: i64 = 60;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConversationId(pub String);

impl EntityId for ConversationId {
    fn get_id_string(&self) -> String {
        self.0.clone()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Conversation {
    turns: u64,
    history: Vec<HistoryEntry>,
    phase: Phase,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum HistoryEntry {
    User(String),
    Bot(String),
    Tool { tool: String, output: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Phase {
    Idle { reset_at: Option<DateTime<Utc>> },
    AwaitingDecision,
    RunningTool { tool: String },
    SendingReply,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ConversationAction {
    UserMessage(String),
    Decided(Decision),
    ToolCompleted { tool: String, output: String },
    ReplySent,
    IdleReset,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Decision {
    Reply(String),
    CallTool { tool: String, args: Vec<String> },
}

pub struct ConversationInit {
    pub id: ConversationId,
}

impl Identified for ConversationInit {
    type Id = ConversationId;
    fn get_id(&self) -> &ConversationId {
        &self.id
    }
}

static CONVERSATION: OnceLock<StateMachineHandle<ConversationMachine>> = OnceLock::new();

pub struct ConversationMachine;

pub fn init_conversation_machine() {
    CONVERSATION
        .set(StateMachineHandle::new(()))
        .ok()
        .expect("ConversationMachine initialized once");
}

impl StateMachine for ConversationMachine {
    type State = Conversation;
    type Id = ConversationId;
    type Action = ConversationAction;
    type Construction = ConversationInit;
    type Env = ();

    fn construct(_init: ConversationInit, _effects: &mut Effects<Self>) -> Conversation {
        Conversation {
            turns: 0,
            history: Vec::new(),
            phase: Phase::Idle { reset_at: None },
        }
    }

    fn transition(
        state: &Conversation,
        id: &ConversationId,
        _env: &Arc<()>,
        action: &ConversationAction,
        effects: &mut Effects<Self>,
    ) -> anyhow::Result<Conversation> {
        conversation_transition(state, id, action, effects)
    }

    fn schedule(state: &Conversation) -> Option<Scheduled<ConversationAction>> {
        match &state.phase {
            Phase::Idle { reset_at: Some(at) } => Some(Scheduled {
                at: *at,
                action: ConversationAction::IdleReset,
            }),
            _ => None,
        }
    }

    fn handle() -> &'static StateMachineHandle<ConversationMachine> {
        CONVERSATION
            .get()
            .expect("ConversationMachine not initialized")
    }
}

fn conversation_transition(
    state: &Conversation,
    id: &ConversationId,
    action: &ConversationAction,
    effects: &mut Effects<ConversationMachine>,
) -> anyhow::Result<Conversation> {
    match (&state.phase, action) {
        (Phase::Idle { .. }, ConversationAction::UserMessage(text)) => {
            let history = with_entry(&state.history, HistoryEntry::User(text.clone()));
            let input = BrainInput::UserText(text.clone());
            effects.enqueue_external(async move { ConversationAction::Decided(decide(input).await) });
            effects.enqueue_action::<StatsMachine>(
                StatsId,
                StatsAction::MessageHandled {
                    conversation: id.0.clone(),
                },
            );
            Ok(Conversation {
                turns: state.turns + 1,
                history,
                phase: Phase::AwaitingDecision,
            })
        }

        (Phase::AwaitingDecision, ConversationAction::Decided(Decision::Reply(text))) => {
            let reply = format!("(turn {}) {}", state.turns, text);
            effects.enqueue_external(send_reply(id.clone(), reply.clone()));
            Ok(Conversation {
                turns: state.turns,
                history: with_entry(&state.history, HistoryEntry::Bot(reply)),
                phase: Phase::SendingReply,
            })
        }

        (Phase::AwaitingDecision, ConversationAction::Decided(Decision::CallTool { tool, args })) => {
            effects.enqueue_external(execute_tool(tool.clone(), args.clone()));
            Ok(Conversation {
                turns: state.turns,
                history: state.history.clone(),
                phase: Phase::RunningTool { tool: tool.clone() },
            })
        }

        (Phase::RunningTool { .. }, ConversationAction::ToolCompleted { tool, output }) => {
            let input = BrainInput::ToolOutput {
                tool: tool.clone(),
                output: output.clone(),
            };
            effects.enqueue_external(async move { ConversationAction::Decided(decide(input).await) });
            Ok(Conversation {
                turns: state.turns,
                history: with_entry(
                    &state.history,
                    HistoryEntry::Tool {
                        tool: tool.clone(),
                        output: output.clone(),
                    },
                ),
                phase: Phase::AwaitingDecision,
            })
        }

        (Phase::SendingReply, ConversationAction::ReplySent) => Ok(Conversation {
            turns: state.turns,
            history: state.history.clone(),
            phase: Phase::Idle {
                reset_at: Some(Utc::now() + Duration::seconds(IDLE_RESET_SECS)),
            },
        }),

        (Phase::Idle { .. }, ConversationAction::IdleReset) => Ok(Conversation {
            turns: 0,
            history: Vec::new(),
            phase: Phase::Idle { reset_at: None },
        }),

        (phase, action) => anyhow::bail!("invalid action {action:?} in phase {phase:?}"),
    }
}

fn with_entry(history: &[HistoryEntry], entry: HistoryEntry) -> Vec<HistoryEntry> {
    history.iter().cloned().chain([entry]).collect()
}
