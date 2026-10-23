# 436 — Work resumed after a restart runs all at once

**Status:** rejected
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Epic:** [378](378-resource-management-epic.md)
**Summary:** A server that went down with many turns under way starts every session with work under way at its start, and all their turns then run at once, however many there are.

## The problem

Since [388](388-resume-interrupted-work-after-a-restart.md) the server starts again, as it starts,
every session in which an agent's work was under way when it last ended
(`SessionManager::resume_interrupted`). It starts them one after another, but starting a session
costs little: it spawns its loop and its actors and returns. What costs is what then runs - every
resumed turn asks its model again, runs its tools, follows its remote tasks - and all of that runs
at once. A server that went down in the middle of a busy hour comes back with every interrupted turn
calling its model in the same second, beside the first requests of the users who come back.

## Why it matters

- The model providers limit the calls per minute; a burst of resumed turns runs into the limit, and
  the users' own turns with it.
- Memory and connections grow with the number of sessions running at once, which
  [381](381-inactive-channels-held-in-memory.md) is about to bound for idle ones.

## Considerations

- **What to bound.** Not the starts, which are cheap, but the sessions with resumed work running at
  once: the next is started once one of those running has no work under way any more. That needs a
  signal that a session's work ended - the actors' states on the event bus, or the state store's
  index (`StateStore::channels_with_work_under_way`) asked again.
- **The order.** The oldest work first, or the sessions of those who come back first - a session a
  user acts in is started anyway, outside the bound.
- **A setting.** How many at once, in `AppConfig`; none meaning as now.
- **A bound on turns in general.** The same burst can come from many users at once without a
  restart; a limit on the model calls running at once would cover both, and would belong to the
  models rather than to the sessions.

## Done when

- After a restart with more interrupted sessions than the bound, no more than the bound of them run
  their resumed work at once, and all of them come to run.

## History

- **2026-10-20** — Opened while implementing [388](388-resume-interrupted-work-after-a-restart.md),
  where the maintainer decided to start the sessions one after another and to bound them for real in
  an issue of its own.
- **2026-10-22** — Rejected by the maintainer: what a restart resumes is the work that was under way
  when the server stopped, no more - the same sessions, the same turns, beside the same users. Its
  memory is what was held before, and its calls to the models are of the order they were before;
  that they begin in the same second is a short peak that spreads out by itself. A bound on the
  model calls running at once would belong to the models and cover every peak, not one of the
  restart; it is not opened as an issue until measurements
  ([307](307-load-and-performance-tests.md)) call for it. A crash caused by a turn would come back
  with its resumed turn, which a bound of the sessions would not prevent either.
