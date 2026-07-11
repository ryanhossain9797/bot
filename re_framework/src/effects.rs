use crate::machine::StateMachine;
use crate::store::OutboxDraft;
use std::future::Future;
use std::pin::Pin;

type Outbound = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Collected during a transition; executed by the runtime only after the transition commits.
/// Two kinds with opposite guarantees (#186): internal entity→entity actions are first-class
/// serialized data, persisted to the outbox atomically with the state change (effectively-once);
/// external effects stay opaque futures, fired once post-commit, never retried (at-most-once).
pub struct Effects<SM: StateMachine> {
    id: SM::Id,
    pub(crate) actions: Vec<OutboxDraft>,
    pub(crate) externals: Vec<Outbound>,
}

impl<SM: StateMachine> Effects<SM> {
    pub(crate) fn new(id: SM::Id) -> Self {
        Effects {
            id,
            actions: Vec::new(),
            externals: Vec::new(),
        }
    }

    pub fn enqueue_action<T: StateMachine>(&mut self, id: T::Id, action: T::Action) {
        self.actions.push(OutboxDraft {
            target_machine: T::name(),
            target_id_json: serde_json::to_string(&id).expect("EntityId serializes"),
            action_json: serde_json::to_string(&action).expect("Action serializes"),
        });
    }

    pub fn enqueue_external(
        &mut self,
        fut: impl Future<Output = SM::Action> + Send + 'static,
    ) {
        let id = self.id.clone();
        self.externals.push(Box::pin(async move {
            let action = fut.await;
            crate::handle::handle::<SM>().act(id, action).await;
        }));
    }
}
