pub enum Envelope<A> {
    Act(A),
    Wakeup(u64),
    Delete,
}
