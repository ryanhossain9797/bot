use crate::lifecycle::{self, Core, Entity, EntityId};
use kameo::message::{Context, Message};
use kameo::{Actor, RemoteActor};
use serde::{Deserialize, Serialize};

pub struct CounterId(pub String);

impl EntityId for CounterId {
    fn id_string(&self) -> String {
        self.0.clone()
    }
}

#[derive(Actor, RemoteActor)]
pub struct Counter {
    pub id: CounterId,
    pub total: i64,
    pub core: Core,
}

#[derive(Serialize, Deserialize)]
pub enum CounterAction {
    Add(i64),
    Reset,
}

pub struct CounterInit {
    pub start: i64,
}

impl Entity for Counter {
    type Id = CounterId;
    type Action = CounterAction;
    type Construction = CounterInit;
    fn construct(id: CounterId, init: CounterInit) -> Self {
        Counter {
            id,
            total: init.start,
            core: Core::default(),
        }
    }
    fn get_core(&self) -> &Core {
        &self.core
    }
    fn with_core(&mut self) -> &mut Core {
        &mut self.core
    }
    fn transition(&mut self, action: CounterAction) {
        match action {
            CounterAction::Add(n) => {
                self.total += n;
                println!("[{}] +{n} -> total={}", self.id.id_string(), self.total);
            }
            CounterAction::Reset => {
                self.total = 0;
                println!("[{}] reset -> total=0", self.id.id_string());
            }
        }
    }
}

#[kameo::remote_message("counter-action")]
impl Message<CounterAction> for Counter {
    type Reply = ();
    async fn handle(&mut self, action: CounterAction, _ctx: &mut Context<Self, ()>) {
        lifecycle::run(self, action);
    }
}
