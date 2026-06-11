use serde::{Deserialize, Serialize};

// The wire type every entity actually receives. Domain actions ride in `Act`; the other variants are
// framework meta-actions the runtime delivers through the same mailbox, so lifecycle and domain share
// one ordered channel. The domain's own `Action` never names these. This is also where a staleness
// epoch will live later (a field alongside the body), gating `Act` against a reset entity.
#[derive(Serialize, Deserialize)]
pub enum Envelope<A> {
    Act(A),
    // A timer firing. Carries no action — the runtime re-evaluates schedule() against current state
    // and fires the fresh action only if overdue. The u64 is the arming GENERATION: a wakeup whose
    // generation no longer matches the entity's current one is from a superseded arm and is dropped,
    // which closes the double-fire window a bare re-evaluation leaves open.
    Wakeup(u64),
    Delete,
}
