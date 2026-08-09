# Large Tasks

For any task big enough that you can't hold the whole thing in your head at once — a multi-file feature, a refactor, a migration, an involved bug hunt. Don't dive straight into editing. Work the loop below.

## 1. Decompose and write the plan down

Before touching anything, break the work into a short sequence of small, individually verifiable steps. Then **write that plan to a file** so it survives — your context can get compacted mid-task and an in-your-head plan is lost when it does.

Write it to **`/work/PLAN.md`** (your workspace is `/work`). Keep it a living document:

```markdown
# <task in one line>

## Goal
<what "done" looks like, concretely>

## Steps
- [ ] 1. <small step> — verify: <how you'll know it worked>
- [ ] 2. ...

## Open questions
- <anything you're unsure about>

## Notes
- <decisions made, dead ends, things learned>
```

Re-read `/work/PLAN.md` whenever you resume or feel lost. Tick boxes as you finish steps and add notes as you learn — future-you (possibly after a context reset) relies on it.

## 2. Verify as you go

Do **one step at a time** and confirm it worked before starting the next — build it, run it, test it, print the output. Don't batch a pile of unverified edits and hope they compose; a regression caught at step 2 is cheap, the same regression discovered at step 9 is not. If a step's verification fails, fix it before moving on, and note what went wrong in the plan.

## 3. Know when to stop and ask

Plans meet reality. Stop and check in with the user — rather than guessing — when:
- a genuine ambiguity or design fork appears that changes the outcome,
- the work turns out much larger or different than expected,
- you'd have to make an irreversible or hard-to-undo change,
- or you're blocked and your fixes aren't landing.

A short "here's what I found, here's the fork, which way?" beats charging ahead on a wrong assumption and redoing it. Surfacing uncertainty early is the sign of doing this well, not badly.
