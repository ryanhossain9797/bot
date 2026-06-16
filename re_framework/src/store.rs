use crate::effects::Effects;
use crate::handle::{run_entity, SoleMailboxHandle};
use crate::machine::StateMachine;
use dashmap::mapref::entry::{Entry as DEntry, OccupiedEntry, VacantEntry};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Our wrapper over the registry entry, so the rest of the crate never names DashMap's types.
pub enum Entry<'a, SM: StateMachine> {
    Occupied(OccupiedSlot<'a, SM>),
    Vacant(VacantSlot<'a, SM>),
}

pub struct OccupiedSlot<'a, SM: StateMachine> {
    inner: OccupiedEntry<'a, String, SoleMailboxHandle<SM>>,
}

pub struct VacantSlot<'a, SM: StateMachine> {
    inner: VacantEntry<'a, String, SoleMailboxHandle<SM>>,
}

pub fn entry<SM: StateMachine>(
    entities: &DashMap<String, SoleMailboxHandle<SM>>,
    id: String,
) -> Entry<'_, SM> {
    match entities.entry(id) {
        DEntry::Occupied(inner) => Entry::Occupied(OccupiedSlot { inner }),
        DEntry::Vacant(inner) => Entry::Vacant(VacantSlot { inner }),
    }
}

impl<SM: StateMachine> OccupiedSlot<'_, SM> {
    pub fn deliver(&self, action: SM::Action) {
        self.inner.get().deliver(action);
    }
}

impl<SM: StateMachine> VacantSlot<'_, SM> {
    pub fn spawn_entity(self, construction: SM::Construction, env: Arc<SM::Env>, id: SM::Id) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut effects = Effects::new(id.clone());
        let state = SM::construct(construction, &mut effects);
        tokio::spawn(run_entity::<SM>(state, rx, env, id, effects));
        self.inner.insert(SoleMailboxHandle { sender: tx });
    }
}
