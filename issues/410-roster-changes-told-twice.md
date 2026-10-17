# 410 — An external agent is told twice that someone joined or left

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Context and knowledge
**Summary:** An external agent whose roster is told `changes_only` learns of a join or a leave twice - from the roster's "Since you were last told who is here" and from the channel's notice in what it is given - and the two can disagree.

## The problem

Two parts of the runtime tell an agent that the roster changed.

**The roster's changes.** `channel_members_prompt_provider` with `changes_only` (issue 204), the
setting for external agents, tells the roster once and then again whenever it changed, followed by
what changed since it was last told (`render_changes`):

```text
Since you were last told who is here:
- Joined: bob (@bob), a person.
```

It compares the roster of now with the one it remembers having told - in memory, for as long as
the component lives, so that after a restart it tells all of it again and no change.

**The channel's notice** (issue [408](408-membership-changes-in-history.md)). Every join and leave is
recorded in the channel's history and in each agent's transcript where it happened, and is given to
the model - or, for an external agent, to its remote agent (`ModelProfile::given_messages`) - as
"Channel notice: @alice added @bob to the channel." among what was said.

An external agent with `changes_only` thus gets both with the same call: the roster with its
"Joined: bob" and, in what was said, the notice of it. The test
`an_external_agent_is_told_who_is_in_its_channel_and_who_joined_or_left_since` shows both.

They do not even say the same:

*   Someone who joined and left between two calls is in neither roster, so the roster's changes say
    nothing; the notices say both.
*   After a restart the roster's changes are lost; the notices are in the transcript and are caught
    up with.
*   The roster's changes do not say who made them, nor where among what was said they happened.

## Why it matters

The same fact told twice in two forms costs tokens on every change, and a model given two accounts
that differ has to reconcile them. The notice is the more complete and the more exact one; the
roster's changes predate it.

## Considerations

*   **Drop the roster's changes** (`render_changes`): with `changes_only` the roster is still told
    again when it changed - the whole of it, with the introductions -, and what changed is told by
    the notices, where it happened. The component no longer needs to remember whom it told, only
    whether the roster changed. Recommended.
*   **Keep both.** Nothing to do; the model reconciles them.
*   **Drop the notices for external agents.** Loses who made the change and where it happened, and
    what happened while the agent was away; and an agent run here, which has no roster's changes,
    keeps the notices anyway.

## Done when

*   An external agent is told of a join or a leave once, by the notice; the roster with
    `changes_only` is told again when it changed, without its own account of what changed.
*   AGENT-TURN.md and COMPONENTS.md say so.

## History

- **2026-10-17** — Issue created, found while implementing issue
  [408](408-membership-changes-in-history.md), which introduced the notices.
- **2026-10-17** — Done as recommended: `render_changes` is gone, and with `changes_only` the roster
  is told again as a whole when it changed; what changed is told by the notices alone. The
  component now remembers the text it told last and compares it, as
  `channel_briefing_prompt_provider` does, rather than the members it named - so a changed
  explanation of how senders are marked is told again, too.
