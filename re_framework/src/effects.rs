use crate::machine::StateMachine;
use std::future::Future;
use std::pin::Pin;

type Outbound = Pin<Box<dyn Future<Output = ()> + Send>>;

pub struct Effects {
    pub(crate) outbound: Vec<Outbound>,
}

impl Effects {
    pub fn none() -> Self {
        Effects {
            outbound: Vec::new(),
        }
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
