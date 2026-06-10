// Concrete kameo-remote entities driven by a generic lifecycle. Each entity embeds a shared
// concrete `Core` (framework bookkeeping) — no generic wrapper needed.
mod counter;
mod lifecycle;

use counter::{Counter, CounterAction, CounterId, CounterInit};
use kameo::message::{Context, Message};
use kameo::remote;
use kameo::{Actor, RemoteActor};
use lifecycle::{Core, Entity, EntityId};
use serde::{Deserialize, Serialize};

pub struct ConvoId(pub String);

impl EntityId for ConvoId {
    fn id_string(&self) -> String {
        self.0.clone()
    }
}

#[derive(Actor, RemoteActor)]
pub struct Convo {
    pub id: ConvoId,
    pub count: i64,
    pub core: Core,
}

#[derive(Serialize, Deserialize)]
pub enum Action {
    Say(String),
}

pub struct ConvoConstruction;

impl Entity for Convo {
    type Id = ConvoId;
    type Action = Action;
    type Construction = ConvoConstruction;
    fn construct(id: ConvoId, _construction: ConvoConstruction) -> Self {
        Convo {
            id,
            count: 0,
            core: Core::default(),
        }
    }
    fn get_core(&self) -> &Core {
        &self.core
    }
    fn with_core(&mut self) -> &mut Core {
        &mut self.core
    }
    fn transition(&mut self, action: Action) {
        match action {
            Action::Say(s) => {
                self.count += 1;
                println!("[{}] '{s}' -> count={}", self.id.id_string(), self.count);
            }
        }
    }
}

#[kameo::remote_message("convo-action")]
impl Message<Action> for Convo {
    type Reply = ();
    async fn handle(&mut self, action: Action, _ctx: &mut Context<Self, ()>) {
        lifecycle::run(self, action);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    remote::bootstrap().map_err(|e| anyhow::anyhow!("bootstrap: {e}"))?;

    lifecycle::construct::<Convo>(ConvoId("convo:1".to_string()), ConvoConstruction).await?;
    lifecycle::act::<Convo>("convo:1", Action::Say("hello".to_string())).await?;
    lifecycle::act::<Convo>("convo:1", Action::Say("again".to_string())).await?;

    lifecycle::construct::<Counter>(CounterId("counter:1".to_string()), CounterInit { start: 10 })
        .await?;
    lifecycle::act::<Counter>("counter:1", CounterAction::Add(5)).await?;
    lifecycle::act::<Counter>("counter:1", CounterAction::Reset).await?;

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    Ok(())
}
