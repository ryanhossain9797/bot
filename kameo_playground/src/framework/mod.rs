pub mod effects;
pub mod envelope;
pub mod runtime;
pub mod traits;

pub use effects::Effects;
pub use envelope::Envelope;
pub use runtime::{act, bootstrap, construct, delete, StateWrapper};
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
        impl ::kameo::message::Message<$crate::framework::Envelope<<$state as $crate::framework::StateMachine>::Action>>
            for $actor
        {
            type Reply = ();
            async fn handle(
                &mut self,
                envelope: $crate::framework::Envelope<<$state as $crate::framework::StateMachine>::Action>,
                ctx: &mut ::kameo::message::Context<Self, ()>,
            ) {
                match envelope {
                    $crate::framework::Envelope::Act(action) => self.0.dispatch(action),
                    $crate::framework::Envelope::Wakeup(generation) => {
                        self.0.on_wakeup(generation)
                    }
                    $crate::framework::Envelope::Delete => {
                        self.0.teardown().await;
                        ctx.stop();
                    }
                }
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
