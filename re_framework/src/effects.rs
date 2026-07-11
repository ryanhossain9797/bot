use crate::machine::StateMachine;
use crate::machine::Identified;
use crate::store::{OutboxDraft, RowKind};
use std::future::Future;
use std::pin::Pin;

type Outbound = Pin<Box<dyn Future<Output = ()> + Send>>;

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
            kind: RowKind::Act,
            target_machine: T::name(),
            target_id_json: serde_json::to_string(&id).expect("EntityId serializes"),
            payload_json: serde_json::to_string(&action).expect("Action serializes"),
        });
    }

    pub fn enqueue_construct<T: StateMachine>(&mut self, construction: T::Construction) {
        self.actions.push(OutboxDraft {
            kind: RowKind::Construct,
            target_machine: T::name(),
            target_id_json: serde_json::to_string(construction.get_id()).expect("EntityId serializes"),
            payload_json: serde_json::to_string(&construction).expect("Construction serializes"),
        });
    }

    pub fn enqueue_act_maybe_construct<T: StateMachine>(
        &mut self,
        construction: T::Construction,
        action: T::Action,
    ) {
        let id = construction.get_id().clone();
        self.enqueue_construct::<T>(construction);
        self.enqueue_action::<T>(id, action);
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
