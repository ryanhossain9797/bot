use crate::effects::Effects;
use crate::handle::{run_entity, Envelope, SoleMailboxHandle};
use crate::machine::StateMachine;
use dashmap::mapref::entry::{Entry, VacantEntry};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

pub fn entry<V>(entities: &DashMap<String, V>, id: String) -> Entry<'_, String, V> {
    entities.entry(id)
}

pub fn spawn_entity<SM: StateMachine>(
    slot: VacantEntry<'_, String, SoleMailboxHandle<SM>>,
    tx: mpsc::UnboundedSender<Envelope<SM::Action>>,
    rx: mpsc::UnboundedReceiver<Envelope<SM::Action>>,
    state: SM::State,
    env: Arc<SM::Env>,
    id: SM::Id,
    effects: Effects<SM>,
) {
    tokio::spawn(run_entity::<SM>(state, rx, env, id, effects));
    slot.insert(SoleMailboxHandle { sender: tx });
}
