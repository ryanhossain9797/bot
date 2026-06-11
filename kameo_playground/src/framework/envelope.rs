// The wire type every entity receives. Domain actions ride in `Act`; the other variants are
// framework meta-actions the runtime delivers through the same mailbox, so lifecycle and domain share
// one ordered channel. The domain's own `Action` never names these.
pub enum Envelope<A> {
    Act(A),
    // A timer firing. Carries no action — the runtime re-evaluates schedule() against current state
    // and fires the fresh action only if overdue. The u64 is the arming GENERATION: a wakeup whose
    // generation no longer matches the entity's current one is from a superseded arm and is dropped.
    Wakeup(u64),
    Delete,
}
