use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use re_framework::{Effects, Scheduled, StateMachine};

use crate::state_machines::conversation_state_machine::ConversationMachine;
use crate::types::conversation::ConversationAction;
use crate::types::reminder::{
    ReminderAction, ReminderConstructor, ReminderForConversation, ReminderState,
};
use crate::Env;

/// A one-shot durable timer scoped to a conversation. Set by the `set_reminder`
/// tool, it schedules a single `Fire` at `fire_at`; when that fires it delivers a
/// system message back into the conversation and moves to a terminal `Fired`
/// state. Modeled on `MemoryManagerMachine`.
pub struct ReminderForConversationMachine;

impl StateMachine for ReminderForConversationMachine {
    type State = ReminderForConversation;
    type Id = crate::types::reminder::ReminderForConversationId;
    type Action = ReminderAction;
    type Construction = ReminderConstructor;
    type Env = crate::Env;

    fn construct(
        constructor: ReminderConstructor,
        _effects: &mut Effects<Self>,
    ) -> ReminderForConversation {
        let created_on = Utc::now();
        let fire_at = created_on + ChronoDuration::seconds(constructor.delay_seconds);
        ReminderForConversation {
            state: ReminderState::Pending,
            conversation_id: constructor.id.conversation_id,
            user_id: constructor.user_id,
            name: constructor.name,
            note: constructor.note,
            created_on,
            fire_at,
        }
    }

    fn transition(
        state: &ReminderForConversation,
        _id: &Self::Id,
        _env: &Arc<Env>,
        action: &ReminderAction,
        effects: &mut Effects<Self>,
    ) -> anyhow::Result<ReminderForConversation> {
        let next_state = match (&state.state, action) {
            (ReminderState::Pending, ReminderAction::Fire) => {
                effects.enqueue_action::<ConversationMachine>(
                    state.conversation_id.clone(),
                    ConversationAction::ReminderFired {
                        note: state.note.clone(),
                        user_id: state.user_id.clone(),
                        name: state.name.clone(),
                    },
                );
                ReminderState::Fired
            }
            _ => return Err(anyhow::anyhow!("no transition for {action:?} in reminder")),
        };

        Ok(ReminderForConversation {
            state: next_state,
            ..state.clone()
        })
    }

    fn schedule(state: &ReminderForConversation) -> Option<Scheduled<ReminderAction>> {
        match &state.state {
            ReminderState::Pending => Some(Scheduled {
                at: state.fire_at,
                action: ReminderAction::Fire,
            }),
            ReminderState::Fired => None,
        }
    }

    fn name() -> &'static str {
        "ReminderForConversationMachine"
    }
}
