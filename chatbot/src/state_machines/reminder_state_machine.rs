use std::sync::Arc;

use re_framework::{Effects, Scheduled, StateMachine, Timestamp};

use crate::Env;
use crate::state_machines::conversation_state_machine::ConversationMachine;
use crate::types::conversation::ConversationAction;
use crate::types::reminder::{
    Reminder, ReminderAction, ReminderConstructor, ReminderFailure, ReminderForConversationId,
    ReminderState,
};

pub struct ReminderForConversationMachine;

fn state_label(state: &ReminderState) -> &'static str {
    match state {
        ReminderState::Pending => "Pending",
        ReminderState::Fired => "Fired",
    }
}

impl StateMachine for ReminderForConversationMachine {
    type State = Reminder;
    type Id = ReminderForConversationId;
    type Action = ReminderAction;
    type Construction = ReminderConstructor;
    type Env = crate::Env;
    type Failure = ReminderFailure;

    fn construct(constructor: ReminderConstructor, _effects: &mut Effects<Self>) -> Reminder {
        Reminder {
            state: ReminderState::Pending,
            conversation_id: constructor.id.conversation_id,
            addressee: constructor.addressee,
            note: constructor.note,
            created_on: Timestamp::now(),
            fire_at: constructor.fire_at,
        }
    }

    fn transition(
        state: &Reminder,
        _id: &Self::Id,
        _env: &Arc<Env>,
        action: &ReminderAction,
        effects: &mut Effects<Self>,
    ) -> Result<Reminder, ReminderFailure> {
        let from = state_label(&state.state);

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
            _ => {
                return Err(ReminderFailure::InvalidAction {
                    action: format!("{action:?}"),
                    state: from.to_string(),
                });
            }
        };

        Ok(Reminder {
            state: next_state,
            ..state.clone()
        })
    }

    fn schedule(state: &Reminder) -> Option<Scheduled<ReminderAction>> {
        match &state.state {
            ReminderState::Pending => Some(Scheduled {
                at: state.fire_at,
                action: ReminderAction::Fire,
            }),
            ReminderState::Fired => None,
        }
    }

    fn name() -> &'static str {
        "ReminderMachine"
    }
}
