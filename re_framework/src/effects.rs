use crate::machine::StateMachine;
use std::future::Future;
use std::pin::Pin;

type Outbound = Pin<Box<dyn Future<Output = ()> + Send>>;

pub struct Effects<SM: StateMachine> {
    id: SM::Id,
    pub(crate) outbound: Vec<Outbound>,
}

impl<SM: StateMachine> Effects<SM> {
    pub(crate) fn new(id: SM::Id) -> Self {
        Effects {
            id,
            outbound: Vec::new(),
        }
    }

    pub fn enqueue_action<T: StateMachine>(&mut self, id: T::Id, action: T::Action) {
        self.outbound.push(Box::pin(async move {
            T::handle().act(id, action);
        }));
    }

    pub fn enqueue_external(
        &mut self,
        fut: impl Future<Output = SM::Action> + Send + 'static,
    ) {
        let id = self.id.clone();
        self.outbound.push(Box::pin(async move {
            let action = fut.await;
            SM::handle().act(id, action);
        }));
    }
}
