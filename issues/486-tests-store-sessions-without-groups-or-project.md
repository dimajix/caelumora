# 486 — Tests of other modules store sessions without their groups or own project

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Beyond the security suites and the tests of sessions and views (issue 471), tests store a session's document - often with its channel, and then run it - without the groups of its participants or its own project, so they test a session the runtime never makes, and whether they need one is nowhere said.

## The problem

Since issue [471](471-world-builds-sessions-by-hand.md), a test that needs a new session stores it
as the runtime does, through `SessionManager::create` or - for one stored, not started -
`session::testing::persist`, which runs the runtime's own step (`SessionManager::persist`): the
session, its own project, its channels, and the group of each one's participants. Other tests still
store the documents by hand, and fall into two kinds:

- **Tests that store a session and its channel, then run it** by `SessionLoop::new`:
  `workspace::testing` (`caelumora-runtime/src/workspace/testing.rs:88`, the fixture of the
  workspaces' tests and of `caelumora-workspace-tools`), and the tests of the agents' prompt parts
  (`caelumora-agents/src/prompt/channel_briefing_prompt_provider.rs:220`,
  `channel_participants_prompt_provider.rs:285`, `channel_language_prompt_provider.rs:203`). Their
  sessions have no groups and no own project; several have default attributes, owned by nobody.
- **Tests that need a session's document alone**, for something beside it:
  `SessionController`'s own tests (`caelumora-runtime/src/controller/session.rs:259`, `:276`,
  `:293`), what the template tests stand in for `create_from_template` with
  (`caelumora-runtime/src/template.rs:253`), the session a channel outside a session reads
  (`caelumora-agents/src/channel_tests/external_agent.rs:137`), and the sessions whose privacy the
  agents' memory tests change (`caelumora-agents/src/components/agent_memory.rs:347`, `:362`).

## Why it matters

- What issue 471 found holds here as well: a step making a session gains is missed by these tests,
  and a check the runtime makes when it founds a session's channels - a participant's clearance -
  is skipped. Whoever reads the groups or the own project in such a test - a view, a question of the
  policy - reads what no session has.
- For the second kind, storing the document alone may be right, but nothing says so: the next
  reader cannot tell a deliberate shortcut from an omission.

## Considerations

- **A. The first kind through `session::testing::persist`** - or `create` where the test runs the
  session anyway - with an owner who is a user. `workspace::testing` and the prompt parts' tests
  then make their `SessionLoop` from the session as stored, or open it by the manager.
- **B. The second kind stays, and says why** in a comment where it stores the document: what the
  test needs of the session, and that it is not a session the runtime makes.
- **C. All of them through `persist`.** One way of storing a session in tests, but the controller's
  and the template's tests would build a manager and an owner for a document they only read.

Recommendation: A and B.

## Done when

- No test stores a session that it then runs or reaches through the views without its groups and
  its own project; a test that stores a session's document alone says why.

## History

- **2026-10-28** — Opened at the maintainer's request from the findings of issue 471.
