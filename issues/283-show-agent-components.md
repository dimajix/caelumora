# 283 — The web UI does not show what an agent was built from

**Status:** open
**Type:** feature
**Priority:** low
**Area:** User interaction
**Summary:** The web UI does not show what an agent was built from, so its owner cannot see which components it has or which came from an included list.

## The problem

With issue [270](270-personality-inheritance.md) a personality includes preconfigured components
by `kind: ref`, and what an agent is made of is no longer written out in its personality: part of
it lies in lists kept elsewhere, which may belong to someone else and change on their own. The
runtime records what each agent was actually built from in a channel, every `ref` resolved
(`MemberView::components`), and the REST server answers it to the agent's owner
(`GET …/actors/{aid}/components`, `ReadAgentComponents`); a member says whether the user may ask
(`can_read_components`). The web UI asks for none of it: an owner cannot see which components their
agent has, nor tell which of them came from an included list.

## Why it matters

- An owner whose agent behaves unexpectedly - a tool missing, a prompt part too many - has no way
  to see what it was built from short of reading the stores.
- A change to an included list reaches an agent only when it is built again; whether a running
  agent already has it cannot be seen.

## Considerations

- The tools panel (`ToolsPanel`, issue 194) is the model: a menu entry on the agent, offered where
  the flag says so ("Show components" for `can_read_components`), opening a panel that reads the
  list from the running actor and says so where there is none - a channel that does not run, a
  former member.
- What to show of a component: its id and kind always; its `params` are configuration - endpoint
  URLs, secret names, prompt texts -, shown to the owner, who may read them, but perhaps folded.
- The response gives the resolved list without saying which `ref` a component came from. Showing
  that would need the server to record it; decide whether it is worth a field.

## Done when

- An agent's owner can open, from the agent's menu, the components it was built from in the
  channel, in their order.
- A user who may not see them is not offered the entry.
- [WEB-UI.md](../docs/WEB-UI.md) describes it.

## History

- **2026-10-09** — Issue created, out of [270](270-personality-inheritance.md): the runtime and the
  REST server show what an agent was built from; the web UI is left to this issue.
