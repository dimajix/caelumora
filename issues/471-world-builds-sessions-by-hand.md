# 471 — The test world stores its sessions by hand instead of making them as the runtime does

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `World::session_of`, which most tests of the security suite start from, and the tests of the views store a session, its groups, its own project and its channels themselves, and so have to be changed whenever making a session gains a step.

## The problem

`World::session_of` (`caelumora-runtime/src/testing/mod.rs:424`) builds a session and its channel
as literals, stores them in the runtime's name, makes the groups of their participants and - since
issue [467](467-projects-hold-what-outlasts-a-session.md) - the session's own project
(`session::make_own`), writes the channel's state with its founders, and only then opens the
session. `SessionManager::create` (test support) and `establish` do the same as the runtime does it.

The tests of the views (`caelumora-runtime/src/view/mod.rs`, `stored_session` and
`stored_channel`, used by 16 tests) do the same for a session that is not running: they store the
session and its channels with the groups of their participants and write the channels' states with
their founders (`session::testing::founded`), "as the runtime does" - but without the own project
467 added, which no session the runtime makes lacks.

## Why it matters

- Every step making a session gains has to be repeated here, or the suite tests a session the
  runtime never makes: 467 had to add the own project by hand, and missed the views' tests.
- What a test asserts about the questions asked (`World::asked`) leaves out what making the session
  asks, which may be right for the tests, but is decided here by accident.

## Considerations

For `World::session_of`:

- **A. Go through `SessionManager::create`**, with the founders given as the channel's
  participants; the session's id and its channel come back from the loop. The lines of `asked` the
  tests compare are taken after it, as now.
- **B. Keep it, and test that it builds what `create` builds** - a test comparing both.

Decision: A, in alice's name: a session is made by a user (`create_from_template` refuses anyone
else), and its founders join in the creator's name ([CHANNELS.md](../docs/CHANNELS.md)) - made in
the runtime's name, every join would read "by system", as in no session the runtime makes.

For the tests of the views, which test what the views show of a session that is not running
(`Views::session` answers a stored one as `Found::Stored`), and build states the runtime reaches
only by several acts - former participants, deleted channels, a history in which someone joined
after something was said, notes and transcripts of agents:

- **C. Make the session with `create`, stop it, then write what the test needs.** Every step of
  making it is the runtime's; but each test runs its actors to have a stored session, its history
  begins with the founders' joins, and what it builds by hand now - former participants, deleted
  channels - would have to be built by the runtime's acts or written over what the runtime stored.
- **D. Split storing a new session off `establish`** - the session, its groups, its own project, its
  channels, rolled back together - into a step of its own, which `establish` runs before announcing
  and starting it, and which `session::testing` offers the views' tests for a session stored, not
  started. What making a session stores stays in one place; what the founders' joins write stays
  `founded`, as for a channel without a session.

Recommendation: D - the views' tests are about stored sessions, and a step of the runtime that
stores one serves them without running anything; C makes each of them longer for no part of what
they test.

## Done when

- `World::session_of` makes its session through the runtime; no test of the suite builds a session's
  groups or project by hand.
- No test of the views stores a session's groups or project by hand.

## History

- **2026-10-28** — Opened at the maintainer's request from the findings of issue 467.
- **2026-10-28** — Step 1, as A: `World::session_of` makes its session with
  `SessionManager::create`, in alice's name, the channel's participants its founders;
  `session::make_own` is private again. Founding asks the clearance the hand-made session skipped:
  the external personality of the agents' suite (`remote_personality`), limited to what is public
  as one that names no roles, was not cleared for the world's internal channel, and now names the
  role of the world's users. The tests of the views, which store their sessions by hand too, were
  added to the issue.
- **2026-10-28** — Step 2, the restructuring for D, without a change of behaviour: storing a new
  session is `SessionManager::persist`, which `establish` runs before announcing and starting it.
- **2026-10-28** — Step 3, done: `session::testing::persist` stores a session with its channels
  through `SessionManager::persist`, with a project of its own, and returns it as stored; the tests
  of the views store theirs with it (`stored`, which also records the founders' joins), as do the
  tests of the manager and of the channels that stored a session and its main channel by hand.
  Tests of other modules still store sessions without their groups or own project - the
  controllers', the workspaces' (`workspace::testing`), the templates', and those of the agents'
  prompt parts, memory and external agents -; they were left as they are.
