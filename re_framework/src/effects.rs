use crate::machine::StateMachine;
use std::future::Future;
use std::pin::Pin;

// An async op whose result loops back as this entity's own next action (the chatbot's "externals").
pub type SelfEffect<SM> = Pin<Box<dyn Future<Output = <SM as StateMachine>::Action> + Send>>;

// A fire-and-forget effect: runs to completion, nothing loops back. Cross-machine sends are this.
type Outbound = Pin<Box<dyn Future<Output = ()> + Send>>;

// What a transition returns alongside the new state. The runtime runs all of these AFTER the new
// state commits, and only on Ok — so an effect can never fire against an uncommitted or rolled-back
// state. Two kinds:
//   - self_actions: futures resolving to this machine's own next action, looped back into it.
//   - outbound: fire-and-forget futures (typically a cross-machine send via `send`).
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

    // Schedule a future whose result is fed back as this machine's own next action.
    pub fn then(mut self, fut: impl Future<Output = SM::Action> + Send + 'static) -> Self {
        self.self_actions.push(Box::pin(fut));
        self
    }

    // Send an action to another machine, fired AFTER this transition commits. Typed: you cannot pair
    // a `T::Id` with the wrong action. Routing goes through `T`'s global handle (`T::handle()`).
    pub fn send<T: StateMachine>(mut self, id: T::Id, action: T::Action) -> Self {
        self.outbound.push(Box::pin(async move {
            T::handle().act(id, action).await;
        }));
        self
    }

    // A general fire-and-forget side effect (no machine target, no loop-back), run after commit.
    pub fn fire(mut self, fut: impl Future<Output = ()> + Send + 'static) -> Self {
        self.outbound.push(Box::pin(fut));
        self
    }
}
