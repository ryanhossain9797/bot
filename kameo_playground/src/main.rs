// Converged shape: generic logic (StateMachine trait) + generic routing, but the kameo actor TYPE
// is concrete per state machine (hand-written with kameo's own macros — remote macros can't be
// generic). No custom macro of ours.
use kameo::actor::{RemoteActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::remote::{self, RemoteMessage};
use kameo::{Actor, RemoteActor};
use serde::{Deserialize, Serialize};

// Generic, reused across state machines.
trait StateMachine: 'static {
    type State: Default + Send + 'static;
    type Msg: Send + 'static;
    fn transition(id: &str, state: &mut Self::State, msg: Self::Msg);
}

// Generic routing — works for ANY concrete entity/message pair.
async fn construct<E>(id: &str, entity: E) -> anyhow::Result<()>
where
    E: Actor<Args = E> + kameo::remote::RemoteActor,
{
    E::spawn(entity).register(id).await?;
    Ok(())
}

async fn act<E, M>(id: &str, msg: M) -> anyhow::Result<()>
where
    E: Actor + kameo::remote::RemoteActor + Message<M> + RemoteMessage<M>,
    M: Serialize + Send + 'static,
{
    if let Some(entity) = RemoteActorRef::<E>::lookup(id).await? {
        entity.tell(&msg).send()?;
    } else {
        println!("[router] no actor for {id}; dropping");
    }
    Ok(())
}

// ---- one state machine: generic logic ----
struct Convo;
#[derive(Default)]
struct ConvoState {
    count: i64,
}
#[derive(Serialize, Deserialize)]
enum ConvoMsg {
    Say(String),
}
impl StateMachine for Convo {
    type State = ConvoState;
    type Msg = ConvoMsg;
    fn transition(id: &str, state: &mut ConvoState, msg: ConvoMsg) {
        match msg {
            ConvoMsg::Say(s) => {
                state.count += 1;
                println!("[{id}] '{s}' -> count={}", state.count);
            }
        }
    }
}

// ---- the concrete entity for `Convo` (the only hand-written boilerplate; kameo macros) ----
#[derive(Actor, RemoteActor)]
struct ConvoEntity {
    id: String,
    state: ConvoState,
}

#[kameo::remote_message("convo-msg")]
impl Message<ConvoMsg> for ConvoEntity {
    type Reply = ();
    async fn handle(&mut self, msg: ConvoMsg, _ctx: &mut Context<Self, ()>) {
        Convo::transition(&self.id, &mut self.state, msg);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    remote::bootstrap().map_err(|e| anyhow::anyhow!("bootstrap: {e}"))?;

    let id = "convo:1";
    construct(
        id,
        ConvoEntity {
            id: id.to_string(),
            state: ConvoState::default(),
        },
    )
    .await?;
    act::<ConvoEntity, _>(id, ConvoMsg::Say("hello".to_string())).await?;
    act::<ConvoEntity, _>(id, ConvoMsg::Say("again".to_string())).await?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    Ok(())
}
