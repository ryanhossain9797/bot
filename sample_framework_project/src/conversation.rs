
use crate::externals::{decide, execute_tool, send_reply, BrainInput};
use crate::stats::{StatsAction, StatsId, StatsInit, StatsMachine};
use chrono::{DateTime, Duration, Utc};
use re_framework::{Effects, EntityId, Identified, Scheduled, StateMachine};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const IDLE_RESET_SECS: i64 = 60;
const RESCUE_SECS: i64 = 30;

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
    phase_since: DateTime<Utc>,
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
    ForceReset,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Decision {
    Reply(String),
    CallTool { tool: String, args: Vec<String> },
}

#[derive(Serialize, Deserialize)]
pub struct ConversationInit {
    pub id: ConversationId,
}

impl Identified for ConversationInit {
    type Id = ConversationId;
    fn get_id(&self) -> &ConversationId {
        &self.id
    }
}

pub struct ConversationMachine;

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
            phase_since: Utc::now(),
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
            Phase::Idle { reset_at: None } => None,
            Phase::AwaitingDecision | Phase::RunningTool { .. } | Phase::SendingReply => Some(Scheduled {
                at: state.phase_since + Duration::seconds(RESCUE_SECS),
                action: ConversationAction::ForceReset,
            }),
        }
    }

    fn name() -> &'static str {
        "ConversationMachine"
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
            effects.enqueue_act_maybe_construct::<StatsMachine>(
                StatsInit { id: StatsId },
                StatsAction::MessageHandled {
                    conversation: id.0.clone(),
                },
            );
            Ok(Conversation {
                turns: state.turns + 1,
                history,
                phase: Phase::AwaitingDecision,
                phase_since: Utc::now(),
            })
        }

        (Phase::AwaitingDecision, ConversationAction::Decided(Decision::Reply(text))) => {
            let reply = format!("(turn {}) {}", state.turns, text);
            effects.enqueue_external(send_reply(id.clone(), reply.clone()));
            Ok(Conversation {
                turns: state.turns,
                history: with_entry(&state.history, HistoryEntry::Bot(reply)),
                phase: Phase::SendingReply,
                phase_since: Utc::now(),
            })
        }

        (Phase::AwaitingDecision, ConversationAction::Decided(Decision::CallTool { tool, args })) => {
            effects.enqueue_external(execute_tool(tool.clone(), args.clone()));
            Ok(Conversation {
                turns: state.turns,
                history: state.history.clone(),
                phase: Phase::RunningTool { tool: tool.clone() },
                phase_since: Utc::now(),
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
                phase_since: Utc::now(),
            })
        }

        (Phase::SendingReply, ConversationAction::ReplySent) => Ok(Conversation {
            turns: state.turns,
            history: state.history.clone(),
            phase: Phase::Idle {
                reset_at: Some(Utc::now() + Duration::seconds(IDLE_RESET_SECS)),
            },
            phase_since: Utc::now(),
        }),

        (Phase::Idle { .. }, ConversationAction::IdleReset) => Ok(Conversation {
            turns: 0,
            history: Vec::new(),
            phase: Phase::Idle { reset_at: None },
            phase_since: Utc::now(),
        }),

        (
            Phase::AwaitingDecision | Phase::RunningTool { .. } | Phase::SendingReply,
            ConversationAction::ForceReset,
        ) => {
            effects.enqueue_external(send_reply(
                id.clone(),
                "(rescued: conversation was stalled mid-flight)".to_string(),
            ));
            Ok(Conversation {
                turns: state.turns,
                history: state.history.clone(),
                phase: Phase::SendingReply,
                phase_since: Utc::now(),
            })
        }

        (phase, action) => anyhow::bail!("invalid action {action:?} in phase {phase:?}"),
    }
}

fn with_entry(history: &[HistoryEntry], entry: HistoryEntry) -> Vec<HistoryEntry> {
    history.iter().cloned().chain([entry]).collect()
}
