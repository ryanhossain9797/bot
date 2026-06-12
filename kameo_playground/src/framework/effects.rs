use super::traits::{EntityId, StateMachine};
use std::future::Future;
use std::pin::Pin;

pub type SelfEffect<S> = Pin<Box<dyn Future<Output = <S as StateMachine>::Action> + Send>>;

pub struct Outbound<T: StateMachine> {
    pub id: T::Id,
    pub message: T::Action,
}

pub trait AnyOutbound: Send {
    fn deliver(self: Box<Self>) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
}

impl<T: StateMachine> AnyOutbound for Outbound<T> {
    fn deliver(self: Box<Self>) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        Box::pin(async move {
            let id = self.id.id_string();
            super::runtime::act::<T>(&id, self.message).await
        })
    }
}

pub struct Effects<S: StateMachine> {
    pub self_actions: Vec<SelfEffect<S>>,
    pub outbound: Vec<Box<dyn AnyOutbound>>,
}

impl<S: StateMachine> Effects<S> {
    pub fn none() -> Self {
        Effects {
            self_actions: Vec::new(),
            outbound: Vec::new(),
        }
    }

    #[allow(dead_code)] 
    pub fn then(mut self, fut: impl Future<Output = S::Action> + Send + 'static) -> Self {
        self.self_actions.push(Box::pin(fut));
        self
    }

    pub fn send<T: StateMachine>(mut self, id: T::Id, message: T::Action) -> Self {
        self.outbound.push(Box::new(Outbound::<T> { id, message }));
        self
    }
}
