# 22 — Managing skills, and who is allowed to use them

**Status:** open
**Area:** Context & knowledge

## The problem

There is no notion of a *skill* anywhere in the workspace. What an agent knows how to do is fixed at
build time by its `Personality.components`: a `static_system_prompt_provider` carries the
instructions, `ToolProvider` components carry the capabilities, and both are decided when the
personality is authored. There is no unit in between — a named, self-contained piece of procedural
knowledge ("how we write a release note", "how this Postgres schema is migrated") that exists
independently of any one personality, can be authored and updated on its own, and is pulled into a
turn only when it is actually relevant.

Two things are missing, and the second is the harder one:

1.  **Management.** Where a skill lives, what it consists of, how it is created, updated, listed and
    versioned.
2.  **Access control.** A skill is not public knowledge. Which agent may load which skill — and on
    whose behalf — has to be an enforced decision, not a convention. `ActionType` has no variant for
    it today.

## Why it matters

Without skills, every capability has to be baked into a personality, which means the instruction set
of an agent grows monotonically and every agent that needs a procedure carries it in its system
prompt for the whole session. That is the same pressure that [16](16-context-compression.md)
describes from the other end. Loading instructions on demand is the cheap half of the answer.

The access-control half matters for a different reason. A skill is an instruction that an agent will
follow. Whoever can put a skill in front of an agent can change what that agent does — for every
user that agent serves. If skills may additionally carry tools, a skill is executable content, and
`PutSkill` becomes a privilege-escalation path rather than an authoring operation. This has to be
decided before the first skill exists, not after, because retrofitting a permission boundary around
a mechanism people already rely on means taking capability away from them.

## Considerations

- **Where skills live.** `EntityStore` is the obvious home: it is where `Personality`,
  `SessionTemplate` and, since the registry moved, `ComponentDescriptor` already live, and it brings
  `Metadata`, `SecurityAttributes` (hence [11](11-agent-ownership.md)) and per-call gating on a
  `SecurityContext` for free. Files on disk would be easier to author by hand but reintroduce a
  second source of truth and have no owner, no scope and no store-level check.
- **How a skill reaches the agent.** The composition point already exists: `StateContextProvider`
  merges any number of `SystemPromptProvider`s into the turn's context. A `skill_provider` component
  that implements `SystemPromptProvider` for the *catalogue* (name and one-line description of every
  skill the caller may load) and `ToolProvider` for a `load_skill` tool that fetches the body on
  demand fits the existing architecture without a new pipeline stage — and progressive disclosure is
  exactly what keeps the always-on cost at one line per skill. The alternative — a new component
  facet, or a fourth stage in the cognitive cycle — buys nothing that these two traits do not
  already give.
- **Two gates, not one.** Authoring (`PutSkill`, `DeleteSkill`) and using (loading a skill into a
  turn) are different decisions with different subjects, and collapsing them into one `ActionType`
  will be wrong in one direction or the other. The using side is the novel one: it is checked
  against the agent *and* the user it acts for, which is precisely the unresolved question in
  [12](12-user-and-agent-permissions.md). Until that rule exists, `UseSkill` has no defensible
  semantics — this issue is blocked on 12 for the runtime half, not for the management half.
- **The catalogue must be filtered by the same check that gates loading.** Listing a skill the
  caller may not load leaks its existence, and worse, invites the model to attempt it and then
  explain the refusal to the user — see [15](15-disallowed-tool-errors-to-llm.md) for why a refusal
  the model cannot act on is its own problem.
- **May a skill carry executable content?** Instructions-only skills are checkable: the worst case
  is bad advice, and the existing tool permissions still bound what follows from it. Skills that
  bundle `ComponentDescriptor`s or tools are far more useful and turn skill authoring into a way to
  inject components into somebody else's agent. If this is allowed at all, the components a skill
  brings must be subject to the same `CallTool` and approval path as any other tool
  ([13](13-approval-modes.md), [14](14-approval-privilege.md)) — a skill must not be a way to arrive
  at an already-approved capability.
- **Scope and sharing.** Global, owner-scoped and session-scoped skills are all plausible, and the
  answer overlaps heavily with [18](18-knowledge-sharing.md) and [19](19-memory-tiers.md): a skill
  an agent derives itself is one concrete shape "knowledge that outlives the session" could take.
  Skills should not become a second, parallel memory mechanism with its own lifetime rules;
  whichever tier model 19 settles on should cover them.
- **Auditability.** Which skills were loaded into a turn is part of what the agent saw when it
  answered, so it belongs to the same reconstruction problem as
  [06](06-context-in-message-for-auditability.md), and the load itself is a security decision that
  belongs in the recorded event stream ([05](05-postgres-event-recorder.md)).
- **Enforcement is vacuous today.** With `DummySecurityManager` approving everything and the REST
  server fabricating its own `SecurityContext` ([10](10-user-authentication.md)), any check added
  here is structure without effect. That is an argument for defining the `ActionType` variants and
  the call sites now — so that authentication, when it lands, has something to enforce — not for
  postponing them.
- **Naming and collisions.** Several skills with the same name from different scopes is the normal
  case, not the exception. `mcp_tool` already solved the analogous problem by namespacing remote
  tools as `"{component_id}__{remote_tool_name}"`; whatever is chosen here should not be a third
  convention.

## Done when

- A `Skill` entity exists with a decided content model — in particular whether a skill may carry
  executable content — stored in the `EntityStore` with `Metadata` and `SecurityAttributes`.
- `ActionType` has separate variants for managing skills and for loading one into a turn, and both
  are checked: the CRUD side in the store/REST path, the loading side in the component that pulls
  the body into the context.
- The skill catalogue offered to the model contains only skills the caller is permitted to load.
- Agents reach skills through the existing `SystemPromptProvider` / `ToolProvider` extension points,
  without a new stage in the cognitive cycle.

## History

- **2026-09-13** — Issue created. Nothing named `skill` exists in the workspace yet, so this is a
  design issue first and an implementation issue second.
