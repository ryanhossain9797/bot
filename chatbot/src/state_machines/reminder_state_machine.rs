use std::sync::Arc;

use chrono::Utc;
use re_framework::{Effects, Scheduled, StateMachine};

use crate::state_machines::conversation_state_machine::ConversationMachine;
use crate::types::conversation::ConversationAction;
use crate::types::reminder::{
    ReminderAction, ReminderConstructor, ReminderForConversation, ReminderForConversationId,
    ReminderState,
};
use crate::Env;

pub struct ReminderForConversationMachine;

impl StateMachine for ReminderForConversationMachine {
    type State = ReminderForConversation;
    type Id = ReminderForConversationId;
    type Action = ReminderAction;
    type Construction = ReminderConstructor;
    type Env = crate::Env;

    fn construct(
        constructor: ReminderConstructor,
        _effects: &mut Effects<Self>,
    ) -> ReminderForConversation {
        ReminderForConversation {
            state: ReminderState::Pending,
            conversation_id: constructor.id.conversation_id,
            addressee: constructor.addressee,
            note: constructor.note,
            created_on: Utc::now(),
            fire_at: constructor.fire_at,
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
                        addressee: state.addressee.clone(),
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
