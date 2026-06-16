use crate::effects::Effects;
use crate::handle::{run_entity, SoleMailboxHandle};
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
    construction: SM::Construction,
    env: Arc<SM::Env>,
    id: SM::Id,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut effects = Effects::new(id.clone());
    let state = SM::construct(construction, &mut effects);
    tokio::spawn(run_entity::<SM>(state, rx, env, id, effects));
    slot.insert(SoleMailboxHandle { sender: tx });
}
