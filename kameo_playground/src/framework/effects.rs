use super::traits::{EntityId, StateMachine};
use std::future::Future;
use std::pin::Pin;

// Side effect #1 (the chatbot's model): an async op that resolves to THIS entity's own next action,
// looped back into it. Opaque — it can do anything, as long as it produces a `Self::Action`.
pub type SelfEffect<S> = Pin<Box<dyn Future<Output = <S as StateMachine>::Action> + Send>>;

// Side effect #2: a message aimed at any addressable entity — a typed (Actor, Id, Message) trio,
// where the "actor" is a target StateMachine `T`, the id is `T::Id`, and the message is `T::Action`.
// NOT an arbitrary future: the framework can route it by id through the registry. The type parameter
// keeps the trio honest — you can't pair a Counter id with a Convo action.
pub struct Outbound<T: StateMachine> {
    pub id: T::Id,
    pub message: T::Action,
}

// Erased so outbounds to different targets collect in one Vec. `deliver` routes the message to its
// target via the registry (`act::<T>`); the concrete `T` is recovered inside the vtable impl, so the
// framework can run a `Vec<Box<dyn AnyOutbound>>` without ever naming any target type.
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

// What a transition returns: zero or more self-action futures, and zero or more typed messages.
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

    // #1: schedule a future that loops its result back as this entity's own action.
    #[allow(dead_code)] // framework surface — the demo exercises send()/the timer, not then()
    pub fn then(mut self, fut: impl Future<Output = S::Action> + Send + 'static) -> Self {
        self.self_actions.push(Box::pin(fut));
        self
    }

    // #2: send a typed message to a target entity (its StateMachine, id, and action).
    pub fn send<T: StateMachine>(mut self, id: T::Id, message: T::Action) -> Self {
        self.outbound.push(Box::new(Outbound::<T> { id, message }));
        self
    }
}
