# kameo_playground

A spike to evaluate migrating the `framework` crate (our hand-rolled state-machine actor
runtime) onto [kameo](https://github.com/tqwewe/kameo). This README is the decision record.

## Verdict: do NOT migrate. Simplify our own `framework` instead.

We built the full architecture on kameo, twice — once on the remote (distributed) path, then
collapsed onto the local (non-remote) path. The conclusion:

- **kameo-local is behaviourally equivalent to what we already have.** Its mailbox is our `mpsc`;
  its transition/effects/schedule model is the one we copied from our framework.
- **kameo's one unique capability — the remote/distributed layer — is unused** (we're single-node)
  **and doesn't solve the hard part of distribution anyway.** Single-activation across machines needs
  sharding/consensus, which kameo does not provide; its DHT registry is eventually-consistent
  *multimap* discovery, not a uniqueness authority.
- Adopting kameo-local actually **cost** us: a dependency, plus registry races we had to hand-handle
  (`remove_by_id`, loser self-drop, stale-entry-on-panic) that our single-owner router never had.

So migrating buys ~nothing today, possibly negative. The spike's real payoff is the design
decisions below, which we will fold into the existing framework. **Revisit kameo only if/when we
genuinely run multiple instances** — and even then it buys transport, not single-activation.

## kameo facts worth remembering (so we don't re-learn them)

- **Three distinct id notions:** kameo's `ActorId` (auto, per-spawn-unique = `peer_id` +
  `sequence_id`) vs the **registry name** (our `id_string`, reusable) vs the **domain id**
  (`ConversationId`). You address/lookup by *name*; there is no public lookup-by-`ActorId`.
- **Two registries:** *local* (`Mutex<HashMap>`, sync, unique-per-name via atomic insert, returns
  `NameAlreadyRegistered`) vs *remote* (Kademlia DHT, async, **multimap**, never rejects a
  duplicate). The `remote` cargo feature decides which `register`/`lookup` you get.
- **No spawn-with-name.** `register` is always a separate step → there is no atomic
  "create-named-or-reject" at either the spawn or the register layer.
- **Local vs remote leaks into the type system** (`ActorRef` vs `RemoteActorRef`, sync vs async
  calls, and the `Serialize`/`Sync` bounds that exist *only* for the wire).
- **Single-activation across nodes is sharding/consensus**, which kameo doesn't provide.

## Design decisions

These are validated by the spike. Tagged by whether the framework already does it (**keep**),
needs it (**add**), or is a structural change (**migrate**).

0. **`Entity` trait (definition) + `StateMachine<E>` parent type** *(migrate)*. Replace the
   framework's fn-pointer wrappers (`Construct` / `Transition` / `Schedule`) and the loose generic
   params on `new_state_machine` with two things:
   - an **`Entity`** trait the domain state implements (`impl Entity for Conversation`), whose
     associated types (`Id`, `Action`, `Construction`, `Env`) and methods (`construct` / `id` /
     `transition` / `schedule`) define one entity;
   - a parent **`StateMachine<E>`** type you hold and operate (`act` / `maybe_construct` / `delete`),
     which wires `E`'s associated types together into the operational API.

   The domain writes `impl Entity for Conversation`; you hold a `StateMachine<Conversation>`. This is
   the "single trait someday" consolidation we parked early on — the spike proved it out, and it
   migrates on its own, independent of the kameo decision. (Naming note: the spike's trait was called
   `StateMachine`; we flip it — the held/operational type takes that name, the per-entity definition
   becomes `Entity`.) Everything below hangs off this split.

1. **Transition is value-in / value-out + `Result`** *(keep)*. Run on a clone; commit the new state
   **and** fire effects **only** on `Ok`. An invalid `(state, action)` pair returns `Err` and is a
   clean no-op (no commit, no effects, no re-arm). **Action staleness is handled here too, as domain
   logic — not a framework concern:** a stale action (e.g. a late `LLMDecisionResult` arriving after
   a `ForceReset`) simply doesn't match the current state and falls through the `_ => Err` arm. The
   state machine *is* the staleness filter; no framework-level epoch/version on actions is needed.
2. **Two kinds of side effect** *(self-actions: keep; outbound: add)*:
   - *self-actions* — futures resolving to this entity's own next action, looped back (today's
     "externals").
   - *outbound* — typed messages to other entities `(StateMachine, Id, Action)`. Not used yet;
     design for it — we will host many entity types.
3. **Timer = absolute deadline + content-free wakeup** *(absolute-time + re-eval: keep; generation
   guard: add)*. `schedule()` returns absolute `DateTime<Utc>` deadlines derived from *stored* state.
   The timer fires a **content-free `Wakeup`**, never the action; on wake, re-evaluate `schedule()`
   against current state and fire the fresh action only if overdue. **Add a per-arm `generation`
   counter and drop wakeups whose generation is stale** — closes a double-fire window the bare
   re-evaluation leaves open.
4. **One timer per entity**, re-armed after each transition; `schedule()` yields the earliest
   (`Vec` and `Option` are equivalent here — only the earliest is ever armed).
5. **Cleanup on *any* stop** *(add)*. On delete, panic, or death: deregister the entity from the
   store **and** abort its timer. Deregister with an **incarnation guard** — remove only if the entry
   is still *this* incarnation (optimistic concurrency, like a SQL `rowversion`) — so a stopping
   entity can't clobber a successor re-created under the same id. Fixes the current bug where a
   panicked entity leaves a stale `BTreeMap` entry and `handle.act`'s `.expect("Send failed")` then
   panics callers.
6. **The entity store lives `Arc`'d inside `StateMachine<E>` — not a router task, not a global**
   *(simplify)*. `StateMachine<E>` owns `Arc<DashMap<E::Id, Entry<E>>>`; it is `Clone` (Arc bumps),
   so every clone shares the same store. `maybe_construct` uses `entry().or_insert_with(spawn)` for
   atomic, race-free get-or-create; `act` does a direct lookup + send (no router funnel). Each
   `Entry` carries the `incarnation` from (5). This removes the single-task router choke-point *and*
   the multimap races kameo forced on us. The **framework stays global-free**; the *domain* stashes
   the one handle it uses in a `OnceCell` for reach (today: a single `StateMachine<Conversation>`).
7. **Implement `DeleteSelf`** *(add)* — currently `todo!()` — with the cleanup from (5).
8. **Per-entity env, carried by `StateMachine<E>`; the framework builds and globals none**
   *(clarify)*. `main` constructs each env (any async setup happens there), builds
   `StateMachine::new(env)`, and stashes it in the domain's `OnceCell`. The handle holds
   `Arc<E::Env>` — no global env registry, no framework-built env.
9. **`Serialize`/`Deserialize` on State, Id, and Action** for persistence-readiness; **not** on `Env`
   (it holds live, unserializable handles rebuilt at startup); **not** `Sync` (that was a remote-only
   artifact — local messaging moves values).

## Deferred (designed, not built)

- **Persistence (#106):** state + outbox committed in **one transaction** (both or rollback). The
  *outbound* effects are persisted in the outbox; the *self-action* externals cannot be serialized,
  so they stay ephemeral and the domain handles retry/skip on recovery. Persistence becomes the
  source of truth.

(Dropped: a framework-level stale-action epoch. Action staleness is domain logic — see decision 1.)

## Next step: simplify the framework

Fold the above into `framework` — no kameo, no new dependency. Backbone first: the `Entity` trait +
`StateMachine<E>` parent (decision 0), since everything else hangs off it. Then the concrete
adds/changes: store `Arc`'d inside `StateMachine<E>` (`DashMap` + `entry()` get-or-create +
incarnation-guarded removal), replacing the router task; on-stop cleanup; implement `DeleteSelf`; add
the wakeup generation guard; design in the outbound effect kind. The domain holds the handle in a
`OnceCell` (one `StateMachine<Conversation>` today). Everything else the framework already does.
