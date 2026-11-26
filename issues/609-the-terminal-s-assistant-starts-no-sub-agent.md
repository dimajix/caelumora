# 609 — The terminal's assistant starts no sub-agent

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The personality `assistant` that `caelumora-tui` writes on its first start has no component `subagents`, and no personality to start a sub-agent of is written beside it, so the terminal's assistant cannot hand a task to a sub-agent, nor can its owner start one by `/subagent`, though the program joins the plugin and the library of `caelumora-subagents`.

## The problem

`caelumora-tui` joins the plugin and the library of sub-agents (`SubagentsPlugin`,
`SubagentsLibrary`, `caelumora-tui/src/main.rs`; issue
[558](558-no-agent-hands-a-task-to-a-sub-agent.md)), so the component kind `subagents` can be
configured in it. But the files it writes on a first start (`FIRST_FILES`,
`caelumora-tui/src/home.rs`) do not use it:

- `defaults/assistant.yaml`, the personality of the default session's assistant, lists no
  component `subagents`: the assistant is offered neither `start_subagent`, `ask_subagent` and
  `end_subagent`, nor its owner the command `/subagent` (issue
  [596](596-a-user-cannot-start-a-sub-agent-by-command.md)).
- No personality to start a sub-agent of is written. The component requires one at least
  (`personalities`, `SubagentsParams::check`), and the program writes `assistant` alone.

Whoever starts the terminal program therefore has to find out that sub-agents exist, write a
personality for them and add the component to `assistant.yaml` by hand before the assistant can
hand off a task and keep its own transcript short.

## Considerations

- **A personality of its own for sub-agents**, not `assistant` itself (decided by the maintainer,
  2026-11-26). A sub-agent works on one brief, apart, and gives back what came of it: its system
  prompt says so, and it needs fewer components than the assistant. Starting `assistant` as its own
  sub-agent would give it a prompt written for talking with the user.
- **Its name.** It is named by an adjective and the personality (`curious-researcher`, TOOLS.md
  *Sub-agents*), so a noun for a role reads best: `helper`, `worker` or `researcher`. `helper`
  suits a general sub-agent of an assistant that works on files and runs commands; `researcher`
  would promise a narrower role than the components give.
- **Its components.** What a sub-agent working on a brief in the workspace needs: a system prompt
  of its own, `workspace`, `turn_end`, `channel_briefing`, `store_places`, `program_log`, `skills`,
  `compaction`, `planning`, `artifacts` - to be checked one by one when it is written:
  - `ask_user`: whether a sub-agent may put questions to the person in its thread, or is to
    answer from its brief alone.
  - `agent_memory` and `notes`: a sub-agent is deleted when it is ended (issue
    [598](598-a-sub-agent-nobody-ends-stays.md)), so memory of its own is lost with it.
  - `subagents`: not given - by default (`plugins.subagents.max_depth: 1`) a sub-agent starts
    none of its own.
- **The component on `assistant`**: `subagents` with `personalities: [<the new one>]`, approval
  as the default (`requires_approval` unset: starting needs none; the personalities listed bound
  what may be started). Whether starting should ask in the terminal is to be decided.
- **Limits** (issue [597](597-sub-agents-are-bounded-by-a-list-of-personalities-alone.md)): the
  new personality has to be limited at least as `assistant` is (`covers_limit`). `assistant` has no
  limit today, so nothing is to do, as long as the two are kept alike.
- **Writing a second personality on the first start.** `Home::first_start` writes each of
  `FIRST_FILES` only where the directory of its kind does not exist yet. A second entry for
  `personalities` would never be written: the first one creates the directory. The rule has to
  become per kind - the files of a kind written together where its directory is missing -, not per
  file, so that what the person removed stays removed.
- **Existing installations** keep their `assistant.yaml` and get neither the component nor the new
  personality: the program never writes them again, by design. The README or the program's
  documentation can say what to add by hand; nothing migrates them.

## Done when

- `caelumora-tui` writes on its first start a personality for sub-agents beside `assistant`, and
  `assistant` has the component `subagents` listing it.
- `first_start` writes every file of a kind whose directory is missing, held by a test.
- A test starts the default session from the files written and finds the tools of
  `caelumora-subagents` offered to the assistant.
- The README says the first start writes the sub-agents' personality too.

## History

- 2026-11-26: Opened. The maintainer asked whether the terminal's assistant can start a sub-agent;
  it cannot, since `assistant.yaml` has no component `subagents`. Decided: a personality of its own
  for sub-agents, not `assistant` itself.
