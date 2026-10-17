# 336 — What a plugin's error says is shown to nobody

**Status:** done
**Area:** Observability and operations
**Summary:** The core turns an error into what a person is told by the error types it knows, so what a plugin's error says is shown as a failure wherever it travels inside an `anyhow`.

## The problem

Two places turn an `anyhow::Error` into what a person is told, both by the error types they know:

- `caelumora_runtime::error::user_message` (issue [02](02-better-error-messages.md)) - what an
  agent's owner, the user of a command or the TUI is shown when a turn, `/wakeup` or `/nudge`
  fails - downcasts the chain to the core's error types it knows to be safe to disclose, and
  answers `None` - "see the server log" - for anything else.
- `From<anyhow::Error> for ApiError` in the REST server answers the core's error types by their
  `From`, a `Refused` anywhere in the chain as `<subject>_forbidden` and a `NameTaken` as
  `409`; everything else as `500 internal_error`.

A plugin's error type is known to neither. Since step 10 of issue
[323](323-kinds-enumerated-in-the-core.md), `SkillError` is a plugin's: where it travels inside an
`anyhow` - a skill that cannot be deleted with its session, an error of the `skills` component
while a turn runs -, its message is shown as a failure. Only what it carries as its source is
still found: a `StoreError`, judged as store errors are, and a `Refused`. Before the move,
`user_message` knew `SkillError` and showed its other messages ("you are not cleared for the
classification of the skill", …).

## Why it matters

Every plugin loses this, and each further one - artifacts, issue
[328](328-artifacts-in-a-crate-of-their-own.md) - loses more. Cut 12 of issue 323 made an error
answered by what it is for the REST handlers that call the plugin directly; it did not reach the
chains.

## Considerations

- **A. A disclosable message as a source**, after the pattern of `Refused`: a type of the core -
  `Disclosed`, say, holding the message a person may read - that an error carries as its
  `#[source]` where its message is safe. `user_message` finds it in the chain as it finds a
  `Refused`. Cheap, and it extends the pattern cut 12 chose; the cost is a field in the variants
  that disclose, and a message written twice where `Display` already says it.
- **B. A trait the error implements** (`UserFacing::user_message`). Not findable in a chain of
  `&dyn Error` without a registry of downcasts, which every plugin would fill in when it
  registers - a second mechanism beside the chain.
- **C. Errors that are safe say so by a marker type wrapping them** (`Disclosable<E>`). Found by
  downcast only if the core knows `E`; does not help.

Recommendation: A, also for the REST server: a disclosed error without a type the server knows is
answered `400` or `409` by a kind the `Disclosed` names, rather than `500`.

## Decisions

Taken on 2026-10-13 by the maintainer.

1. **A plugin answers the core in the core's types.** A, B and C all try to find a plugin's error
   inside an `anyhow` after the fact. A misuses the chain: `#[source]` says what caused an error,
   and `Disclosed` would be no cause but a property of the error carrying it - smuggled into the
   chain so that a downcast finds it. B needs a registry beside the chain, C does not help. What is
   wanted instead is an error the core knows at the point where the plugin answers the core. Where
   that is - a facet of its component, `ScopeMember` - the trait names an error type of the core,
   and the plugin translates its own into it, deciding there what may be shown, as
   `SkillsComponent` already does for `ToolError`, `CommandError` and `CompactionError`. The
   compiler then asks for the translation; a `?` into an `anyhow` cannot forget it. `Disclosed` is
   not made; `user_message` and `From<anyhow::Error> for ApiError` stay as they are. The rule is
   written down in `docs/PLUGINS.md`.
2. **Where `SkillError` really reaches an `anyhow`** - checked while deciding: only in
   `PromptProvider::get_prompt` (`usable_skills().await?` in `caelumora-skills/src/component.rs`).
   The example under *The problem*, a skill that cannot be deleted with its session, is none:
   `SkillManager`'s `ScopeMember` meets only `StoreError`s, which `user_message` judges already.
3. **`ScopeMember` answers `StoreResult`**, in all three of its methods (question 1 below, as
   recommended). `purge_lying_in` included: the owner turns a failure to remove the place or the
   content behind an entity into `StoreError::Backend`.
4. **The context-building facets answer `caelumora_runtime::context::ContextError`**, one type for
   `PromptProvider::get_prompt` and `ContextProvider::current_context` (question 2). The name is
   taken from the views: `view::ContextError` - why the context an agent was given for a message
   cannot be shown, made by `AgentContextView::context_of` alone - is renamed `AgentContextError`,
   after its maker. The two are not merged into one type: they share no variant, and they reach
   different ends - a recorded context read afterwards is answered `404` over REST, a context that
   cannot be built ends the turn. One type would make each caller handle the variants of the other,
   which never reach it. The REST codes `context_not_recorded` and `context_unavailable` stay.
5. **`MessageProcessor` answers an error type of its own**, in `message.rs`. Question 2 counted it
   among the context-building facets, which it is not: it is a stage an actor's messages pass
   (`PerceiveInProcessor`, `EmitOutProcessor`), and putting it into `ContextError` would mix two
   subjects as merging with `AgentContextError` would. It gets a type of its own now rather than
   staying `anyhow` until a plugin implements it, so that the rule of `docs/PLUGINS.md` has no
   exception. `ComponentImpl::apply_fail_policy`, which takes an `anyhow::Result` today, becomes
   generic over the error type, for these facets as for the others it serves.
6. **Each type keeps its own "failed" variant** (question 3): `ToolError`, `CommandError`,
   `CompactionError`, and the two new types one each. Each is the answer of one facet; no common
   type is made, and no issue opened for it.

## The boundaries that answer `anyhow`

What a plugin implements, or will, and that still answers `anyhow` while the runtime runs:

- `PromptProvider::get_prompt` - the skills' catalogue; the one place where a `SkillError` is lost
  today.
- `ContextProvider::current_context` and `MessageProcessor::process_messages` - no plugin
  implements them yet, but they are facets like the others.
- `ScopeMember`, checked against its six members (`Unowned`, agents, workspaces, artifacts, groups,
  skills):
  - `delete_lying_in` and `restore_lying_in` meet nothing but `StoreError`s in every member: the
    listing, and the deletion or restoring through `deleted_unless_refused` and
    `restored_unless_refused`, which answer `StoreResult`. They can answer `StoreResult` as they
    are.
  - `purge_lying_in` meets more: the agents' state store (`StoreError`), but also the workspaces'
    `WorkspaceError` (`provider`) and `TerminalError` (removing the place), and the artifacts'
    `ArtifactError` (`storage`, removing the content) - which becomes a plugin's with issue
    [328](328-artifacts-in-a-crate-of-their-own.md).

`Plugin::register` and `SeedSection::seed` stay `anyhow`: they run at startup, and what fails there
ends the program and goes to the log.

## Open questions

1. **What `purge_lying_in` answers.** (a) `StoreResult`, the owner turning a failure to remove the
   place or the content behind an entity into `StoreError::Backend` - which means "what the storage
   itself reported", is never shown and is a server error over REST, as such a failure should be;
   the cost is that the cause becomes a string. (b) An error type of the core for purging. (a) is
   recommended: what lies behind an entity is its storage, and nothing a purge reports beyond the
   store's own errors is for a person to read.
2. **The type of the context-building facets.** `get_prompt`, `current_context` and
   `process_messages` need an error type of the core. One type for all three - what building a
   turn's context failed with - rather than one each is recommended; its name is open
   (`ContextError` is taken by the views).
3. **Four "failed, with a reason that may be shown" variants.** `ToolError`, `CommandError` and
   `CompactionError` each have one, and the type of question 2 would bring a fourth. Whether that is
   a deliberate difference or repetition to fold into one type of the core is a question of its own,
   possibly an issue of its own.

## Done when

- `PromptProvider` and `ContextProvider` answer `context::ContextError`, `MessageProcessor` a type
  of its own, `ScopeMember` `StoreResult`; the view's `ContextError` is `AgentContextError`.
- Every other trait of the core that a plugin implements and that is called while the runtime runs
  names an error type of the core as well, and no `SkillError` reaches an `anyhow`.
- `docs/PLUGINS.md` says that a plugin answers the core in the core's types, and why.

## History

- **2026-10-12** — Opened from step 10 of issue 323: moving skills into a plugin left
  `user_message` without `SkillError`.
- **2026-10-13** — Decided: no `Disclosed` and no trait found in the chain; the traits a plugin
  implements name an error type of the core instead (see *Decisions*). Checked where `SkillError`
  reaches an `anyhow` (only `get_prompt`) and what `ScopeMember`'s members meet; the rule written
  down in the new `docs/PLUGINS.md`, which takes over the section on plugins from
  `docs/COMPONENTS.md`. *Done when* rewritten to match the decision.
- **2026-10-13** — Open questions decided (*Decisions* 3 to 6): `StoreResult` for `ScopeMember`,
  `context::ContextError` for the context-building facets with the view's renamed
  `AgentContextError`, a type of its own for `MessageProcessor`, a "failed" variant in each type.
- **2026-10-13** — Done. The view's `ContextError` is `AgentContextError`. `ScopeMember` answers
  `StoreResult`; the workspaces and the artifacts turn a place or a content that cannot be removed
  into `StoreError::Backend`, and `ScopedEntities` restores and purges in `StoreResult` too.
  `PromptProvider` and `ContextProvider` answer `context::ContextError` (`Failed`, `View`, `Store`,
  and `Component`, naming a prompt provider that failed closed), `MessageProcessor`
  `message::ProcessingError` (`Failed`, `View`, `Store`); `user_message` judges both by their
  variants. `ComponentImpl::apply_fail_policy` became generic over the error type and no longer
  names the component: the caller does, in its own type - `ComponentImpl<dyn
  PromptProvider>::prompt_for` for both context providers, `apply_stage_processors` with an
  `anyhow` context as before. The skills' catalogue translates its `SkillError` with
  `From<SkillError> for ContextError`. Correction to decision 6 and question 3: `CompactionError`
  has no "failed, with a reason" variant - its variants are fixed reasons -, so the decision holds
  for `ToolError`, `CommandError` and the two new types. No stored data changes.
