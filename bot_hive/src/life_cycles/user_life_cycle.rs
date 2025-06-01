use std::{future::Future, pin::Pin, sync::Arc};

use crate::{
    models::user::{User, UserAction, UserId, UserState},
    Env,
};
use chrono::{Duration as ChronoDuration, Utc};
use lib_hive::{ExternalOperation, Scheduled, TransitionResult};

use crate::connectors::user_connector::handle_bot_message;

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

pub fn user_transition(
    env: Arc<Env>,
    user_id: UserId,
    user: User,
    action: &UserAction,
) -> Pin<Box<dyn Future<Output = UserTransitionResult> + Send + '_>> {
    Box::pin(async move {
        match (user.state, action) {
            (
                UserState::Idle,
                UserAction::NewMessage {
                    msg,
                    start_conversation: true,
                },
            ) => {
                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(handle_bot_message(
                    env.clone(),
                    user_id.clone(),
                    format!("You said {msg}"),
                )));

                let user = User {
                    state: UserState::RespondingToMessage,
                };

                println!("Id: {0} {1:?}", user_id.1, user.state);

                Ok((user, external))
            }
            (UserState::RespondingToMessage, UserAction::SendResult(send_result)) => Ok((
                User {
                    state: UserState::WaitingToSayGoodbye(Some(
                        Utc::now() + ChronoDuration::milliseconds(10_000),
                    )),
                    ..user
                },
                Vec::new(),
            )),
            (UserState::WaitingToSayGoodbye(_), UserAction::Poke) => {
                println!("Poked");

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(handle_bot_message(
                    env.clone(),
                    user_id.clone(),
                    "Goodbye".to_string(),
                )));

                Ok((
                    User {
                        state: UserState::SayingGoodbye,
                    },
                    external,
                ))
            }
            (UserState::SayingGoodbye, UserAction::SendResult(_)) => Ok((
                User {
                    state: UserState::Idle,
                },
                Vec::new(),
            )),
            _ => Err(anyhow::anyhow!("Invalid state or action")),
        }
    })
}

pub fn schedule(user: &User) -> Vec<Scheduled<UserAction>> {
    match user.state {
        UserState::WaitingToSayGoodbye(Some(poke_at)) => {
            vec![Scheduled {
                at: poke_at,
                action: UserAction::Poke,
            }]
        }
        _ => Vec::new(),
    }
}
