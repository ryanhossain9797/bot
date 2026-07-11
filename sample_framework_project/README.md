# sample_framework_project

Deterministic harness for the `re_framework` persistence refactor
([#186](https://github.com/ryanhossain9797/bot/issues/186)). A minimal replica of the
chatbot — same four-phase conversation machine (Idle / AwaitingDecision / RunningTool /
SendingReply), same decision loop — but the LLM is replaced by a deterministic fake brain
and the tools by one fake `add` tool. The state machine layer doesn't know the difference:
the LLM is just an external effect, so this exercises the identical framework surface with
none of the model/GPU/Discord weight.

Runs natively, no containers:

```
cargo run -p sample_framework_project
```

Input grammar (stdin):

```
hello                     → [main] (turn 1) echo: hello
work: tool add 2 3        → [work] (turn 1) add returned: 5
exit                      → quit
```

This package is also the framework's failure-scenario harness: `tests/framework_scenarios.rs`
encodes the findings of the #188 adversarial review (construct double-dispatch, stale-seq
redelivery, rows wedged behind live actors, poison-on-unregistered-machine, delete/recreate
dedup corruption, name collisions) as regression tests, injecting crash leftovers straight
into the SQLite file. Run with `cargo test -p sample_framework_project`.

State lives in `./framework_db/sample.db` (Turso; standard SQLite file — inspectable with
`python3 -c "import sqlite3; ..."`). Tables: `entities` (state + version + outbox seq),
`outbox` (durable entity→entity actions), `call_dedup` (receiver-side idempotency).

What to try:

- **Persistence/resume** — send a few messages, kill the process, run again, send another:
  the turn counter continues.
- **Crash recovery of internal actions** — every user message sends a durable action to the
  singleton `StatsMachine` through the transactional outbox. Kill the process between a
  conversation's commit and the stats delivery (or inject a row into `outbox` while stopped):
  on the next start, `[sweep] woke N entities` redispatches it and stats catches up.
  Redeliver the same row twice and dedup absorbs it — no double-count.
- **CAS as the transaction guarantee** — edit an entity's `version` in the DB while the app
  runs and send it a message: the write conflicts, the actor logs `CAS CONFLICT` and drops,
  and the next message rebuilds it from the store.
- **Timers** — an idle conversation resets after 60s via `schedule()`; the deadline is part
  of persisted state, so it re-arms (or fires immediately) after a restart.
- **Externals lost, rescue timer catches it** — external effects (`decide`, `send_reply`,
  `execute_tool` here; the LLM/Discord in the real bot) are at-most-once and never retried.
  Kill mid-decision and the in-flight work is gone — but every busy phase carries a 30s
  `ForceReset` rescue timer, `next_tick_on` is persisted, and the background sweep force-wakes
  due entities. So the stranded conversation announces "(rescued: …)" and returns to service
  **without any user contact** — even across the restart. (Try it: stop the app, set `main`'s
  phase to `AwaitingDecision` with an old `phase_since`/`next_tick_on` in the DB, start the
  app, and just wait.)
