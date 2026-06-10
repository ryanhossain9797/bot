pub mod effects;
pub mod runtime;
pub mod traits;

pub use effects::Effects;
pub use runtime::{act, bootstrap, construct, StateWrapper};
pub use traits::{Entity, EntityId, Env, Scheduled, StateMachine};

// Generates the per-entity concrete glue from a pure domain state: the kameo actor newtype around
// StateWrapper<State>, its remote_message handler, and the Entity impl. Pure sugar — the type
// system (StateMachine::Wrapped: Entity) is what actually enforces the contract.
#[macro_export]
macro_rules! entity {
    ($actor:ident, $state:ty, $remote_id:literal) => {
        #[derive(::kameo::Actor, ::kameo::RemoteActor)]
        pub struct $actor($crate::framework::StateWrapper<$state>);

        #[::kameo::remote_message($remote_id)]
        impl ::kameo::message::Message<<$state as $crate::framework::StateMachine>::Action>
            for $actor
        {
            type Reply = ();
            async fn handle(
                &mut self,
                action: <$state as $crate::framework::StateMachine>::Action,
                _ctx: &mut ::kameo::message::Context<Self, ()>,
            ) {
                self.0.dispatch(action);
            }
        }

        impl $crate::framework::Entity for $actor {
            type State = $state;
            fn build(
                id: <$state as $crate::framework::StateMachine>::Id,
                construction: <$state as $crate::framework::StateMachine>::Construction,
            ) -> Self {
                $actor($crate::framework::StateWrapper::new(
                    <$state as $crate::framework::StateMachine>::construct(id, construction),
                ))
            }
            fn id_string(&self) -> String {
                self.0.id_string()
            }
        }
    };
}
