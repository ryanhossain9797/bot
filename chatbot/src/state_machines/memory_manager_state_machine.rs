use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use re_framework::{Effects, Scheduled, StateMachine};

use crate::externals::summarize_external::summarize;
use crate::state_machines::conversation_state_machine::ConversationMachine;
use crate::types::conversation::{ConversationAction, ConversationId, InterruptionReason};
use crate::types::memory::{
    MemoryManager, MemoryManagerAction, MemoryManagerConstructor, MemoryManagerState,
};
use crate::Env;

const COMPACT_TIMEOUT_MS: i64 = 300_000;

pub struct MemoryManagerMachine;

impl StateMachine for MemoryManagerMachine {
    type State = MemoryManager;
    type Id = ConversationId;
    type Action = MemoryManagerAction;
    type Construction = MemoryManagerConstructor;
    type Env = crate::Env;

    fn construct(
        _constructor: MemoryManagerConstructor,
        _effects: &mut Effects<Self>,
    ) -> MemoryManager {
        MemoryManager {
            state: MemoryManagerState::Idle,
            last_transition: Utc::now(),
        }
    }

    fn transition(
        state: &MemoryManager,
        id: &ConversationId,
        env: &Arc<Env>,
        action: &MemoryManagerAction,
        effects: &mut Effects<Self>,
    ) -> anyhow::Result<MemoryManager> {
        let next_state = match (&state.state, action) {
            (MemoryManagerState::Idle, MemoryManagerAction::Compact { history }) => {
                effects.enqueue_external(summarize(Arc::clone(env), history.clone()));
                MemoryManagerState::Compacting
            }
            (MemoryManagerState::Compacting, MemoryManagerAction::CompactionDone(result)) => {
                effects.enqueue_action::<ConversationMachine>(
                    id.clone(),
                    ConversationAction::CompactionResult(result.clone()),
                );
                MemoryManagerState::Idle
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "no transition for {action:?} in memory manager"
                ))
            }
        };

        Ok(MemoryManager {
            state: next_state,
            last_transition: Utc::now(),
        })
    }

    fn schedule(state: &MemoryManager) -> Option<Scheduled<MemoryManagerAction>> {
        match &state.state {
            MemoryManagerState::Idle => None,
            MemoryManagerState::Compacting => Some(Scheduled {
                at: state.last_transition + ChronoDuration::milliseconds(COMPACT_TIMEOUT_MS),
                action: MemoryManagerAction::CompactionDone(Err(InterruptionReason::TimedOut)),
            }),
        }
    }

    fn name() -> &'static str {
        "MemoryManagerMachine"
    }
}
