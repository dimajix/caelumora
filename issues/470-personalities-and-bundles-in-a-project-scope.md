# 470 — Personalities and component bundles are stored in a project's scope, where nothing uses them

**Status:** open
**Type:** bug
**Priority:** low
**Area:** Code structure
**Summary:** A personality or a component bundle can be stored in a project's scope, though nothing is used from there yet, while session templates, agents, workspaces, skills and artifacts are each refused or placed by their own validation.

## The problem

Since issue [467](467-projects-hold-what-outlasts-a-session.md) a project holds a scope of its own
(`Scope::ProjectScope`), and nothing lying there is used at a place: `Scope::admits` answers
`false` for it. Each kind that could lie there says so in its `Stored::validate`: a session
template (`caelumora-model/src/template.rs:300`), an agent (`agent/mod.rs:129`), a workspace
(`workspace.rs`), a skill (`caelumora-skills/src/skill.rs:129`) are refused, an artifact lies there
alone. A personality (`caelumora-model/src/agent/personality.rs`) and a component bundle
(`caelumora-model/src/component.rs`) have no validation of their scope at all, so they can be made
in a project's scope - and in a channel's - and are then found by no lookup.

The tests of 467 even use a component bundle in a project's scope to stand for "something that lies
there" (`a_session_s_own_project_goes_and_comes_back_with_it`, the security suite's
`a_session_s_own_project_is_read_by_its_participants_and_goes_with_it`).

## Why it matters

What is stored where it is never found is a trap: whoever makes it sees it saved and never used. And
the rule "what lies in a project's scope is used nowhere yet" holds by the validation of each kind
but these two.

## Considerations

- **A. Refuse it now**, in `validate` of both kinds, as the others are refused; the tests of 467
  that use a bundle there take an artifact instead (which needs the artifacts plugin) or a kind of a
  test of their own.
- **B. Leave it to the step "lookups from a place"** (PROJECTS.md, *Not done yet*), which decides
  whether personalities and bundles lie in projects at all, and gives `admits` the chain of a
  place - then the validation follows what is decided there.
- Either way, a channel's scope is to be looked at for both kinds as well.

Recommendation: B, with this issue as its reminder; A if the trap is met before.

## Done when

- What a personality and a component bundle may lie in is decided, and checked by their own
  validation as every other kind's is.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 5 of issue 467.
