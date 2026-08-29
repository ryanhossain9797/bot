use std::sync::Arc;

use re_framework::{Effects, Scheduled, SignedDuration, StateMachine, Timestamp};

use crate::Env;
use crate::externals::summarize_external::summarize;
use crate::state_machines::conversation_state_machine::ConversationMachine;
use crate::types::conversation::{ConversationAction, ConversationId, InterruptionReason};
use crate::types::memory_manager::{
    MemoryManager, MemoryManagerAction, MemoryManagerConstructor, MemoryManagerFailure,
    MemoryManagerState,
};

const COMPACT_TIMEOUT_MS: i64 = 300_000;

fn state_label(state: &MemoryManagerState) -> &'static str {
    match state {
        MemoryManagerState::Idle => "Idle",
        MemoryManagerState::Compacting => "Compacting",
    }
}

pub struct MemoryManagerMachine;

impl StateMachine for MemoryManagerMachine {
    type State = MemoryManager;
    type Id = ConversationId;
    type Action = MemoryManagerAction;
    type Construction = MemoryManagerConstructor;
    type Env = crate::Env;
    type Failure = MemoryManagerFailure;

    fn construct(
        _constructor: MemoryManagerConstructor,
        _effects: &mut Effects<Self>,
    ) -> MemoryManager {
        MemoryManager {
            state: MemoryManagerState::Idle,
            last_transition: Timestamp::now(),
        }
    }

    fn transition(
        state: &MemoryManager,
        id: &ConversationId,
        env: &Arc<Env>,
        action: &MemoryManagerAction,
        effects: &mut Effects<Self>,
    ) -> Result<MemoryManager, MemoryManagerFailure> {
        let from = state_label(&state.state);

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
                return Err(MemoryManagerFailure::InvalidAction {
                    action: format!("{action:?}"),
                    state: from.to_string(),
                });
            }
        };

        Ok(MemoryManager {
            state: next_state,
            last_transition: Timestamp::now(),
        })
    }

    fn schedule(state: &MemoryManager) -> Option<Scheduled<MemoryManagerAction>> {
        match &state.state {
            MemoryManagerState::Idle => None,
            MemoryManagerState::Compacting => Some(Scheduled {
                at: state.last_transition + SignedDuration::from_millis(COMPACT_TIMEOUT_MS),
                action: MemoryManagerAction::CompactionDone(Err(InterruptionReason::TimedOut)),
            }),
        }
    }

    fn name() -> &'static str {
        "MemoryManagerMachine"
    }
}
