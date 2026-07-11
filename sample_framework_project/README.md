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

What to try:

- **Persistence/resume** — send a few messages, kill the process, run again, send another:
  the turn counter continues (state reloaded from `./framework_db/`).
- **Timers** — an idle conversation resets after 60s via `schedule()`; the deadline is part
  of persisted state, so it re-arms (or fires immediately) after a restart.
- **Entity→entity** — every user message also sends an action to the singleton
  `StatsMachine`, which prints running counts.
- **Today's crash gap (what #186 fixes)** — kill the process while a decision/tool is
  in flight (phase ≠ Idle): the in-flight effect is lost and, on restart, the conversation
  is wedged in that phase. With the transactional outbox this becomes recoverable; this
  harness is where that fix gets proven.
