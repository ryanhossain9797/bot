use crate::machine::StateMachine;
use std::future::Future;
use std::pin::Pin;

pub type SelfEffect<SM> = Pin<Box<dyn Future<Output = <SM as StateMachine>::Action> + Send>>;

type Outbound = Pin<Box<dyn Future<Output = ()> + Send>>;

pub struct Effects<SM: StateMachine> {
    pub(crate) self_actions: Vec<SelfEffect<SM>>,
    pub(crate) outbound: Vec<Outbound>,
}

impl<SM: StateMachine> Effects<SM> {
    pub fn none() -> Self {
        Effects {
            self_actions: Vec::new(),
            outbound: Vec::new(),
        }
    }

    pub fn then(mut self, fut: impl Future<Output = SM::Action> + Send + 'static) -> Self {
        self.self_actions.push(Box::pin(fut));
        self
    }

    pub fn send<T: StateMachine>(mut self, id: T::Id, action: T::Action) -> Self {
        self.outbound.push(Box::pin(async move {
            T::handle().act(id, action).await;
        }));
        self
    }

    pub fn fire(mut self, fut: impl Future<Output = ()> + Send + 'static) -> Self {
        self.outbound.push(Box::pin(fut));
        self
    }
}
