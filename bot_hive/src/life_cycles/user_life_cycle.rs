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
                UserState::Idle(_),
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
                    state: UserState::SendingMessage,
                };

                println!("Id: {0} {1:?}", user_id.1, user.state);

                Ok((user, external))
            }
            (UserState::SendingMessage, UserAction::SendResult(_)) => Ok((
                User {
                    state: UserState::Idle(Some(Utc::now())),
                    ..user
                },
                Vec::new(),
            )),
            (UserState::Idle(Some(_)), UserAction::Timeout) => {
                println!("Timed Out");

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(handle_bot_message(
                    env.clone(),
                    user_id.clone(),
                    "Goodbye".to_string(),
                )));

                Ok((
                    User {
                        state: UserState::SendingMessage,
                    },
                    external,
                ))
            }
            _ => Err(anyhow::anyhow!("Invalid state or action")),
        }
    })
}

pub fn schedule(user: &User) -> Vec<Scheduled<UserAction>> {
    match user.state {
        UserState::Idle(Some(last_activity)) => {
            vec![Scheduled {
                at: last_activity + ChronoDuration::milliseconds(10_000),
                action: UserAction::Timeout,
            }]
        }
        _ => Vec::new(),
    }
}
