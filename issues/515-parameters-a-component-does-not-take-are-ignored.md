# 515 — Parameters a component kind does not take are ignored without a word

**Status:** open
**Type:** bug
**Priority:** low
**Area:** Observability and operations
**Summary:** A component kind without parameters builds whatever `params` its descriptor gives and ignores them, so a parameter misspelt, set on the wrong component or moved to another is lost without a word, while a kind with parameters refuses what it does not know.

## The problem

A component kind that takes parameters reads them as a struct of its own with
`config_of_params`, and the struct is `#[serde(deny_unknown_fields)]`: a parameter it does not
know is refused when the component is built (`ComponentError::InvalidConfig`), naming it. A kind
without parameters does not read its `params` at all; its factory ignores the descriptor
(`|_descriptor, context|`). About twenty kinds are written that way, among them
`summary_compaction`, `notes`, `channel_history`, `create_plan_tool`, `stop_tool`, `ask_user_tool`,
`help_command` and the moderator policies.

Issue [413](413-compaction-restores-unused-skills.md) made the difference visible: `keep` moved
from `summary_compaction` to `compaction`. Until then, a `keep` set on `summary_compaction` was
read; since, it is ignored, and the agent keeps the default share of its context without anyone
being told. Before 413, `summary_compaction` took `keep` and would have refused a parameter it did
not know.

## Why it matters

*   A mistake in a personality - a parameter misspelt, set on the wrong component, or one a
    change moved elsewhere - has no effect and no message; whoever configured it believes it
    holds.
*   Two kinds answer the same mistake differently, depending only on whether they happen to take
    parameters today. A kind that gains its first parameter starts refusing what it ignored
    before.
*   The rule "stored data is not kept compatible, the summary of a change says what to delete"
    relies on a stored configuration that no longer fits failing loudly.

## Considerations

*   **The registry checks, for every kind.** A kind declares what it takes - a configuration type
    with its kind (`ComponentKind`), or that it takes none -, and the registry refuses unknown
    parameters before the factory runs, in one place. The factories of kinds without parameters
    stay as they are. A change to `ComponentKind` and `ComponentRegistry` in the core.
*   **Every kind without parameters reads an empty configuration**: `config_of_params` with a
    struct without fields, `deny_unknown_fields`. Small, but the same lines in twenty factories,
    and a new kind that forgets them is back to ignoring.
*   **As it is**, the difference named in COMPONENTS.md. Leaves the mistake silent.

Recommended: the first - one rule in one place, as `deny_unknown_fields` already is for the kinds
with parameters. To check first: whether any configuration in use, the seed's
(`examples/seed.yaml`) or a stored personality, gives parameters to a kind that takes none - it
would no longer build, and its agent no longer start.

## Done when

*   A parameter that a component kind does not take is refused when the component is built, naming
    the component and the parameter, whether the kind takes parameters or not; a test covers a kind
    without parameters.

## History

- **2026-10-30** — Opened after issue 413, which moved `keep` from `summary_compaction` to
  `compaction`: a `keep` still set on `summary_compaction` is ignored since.
