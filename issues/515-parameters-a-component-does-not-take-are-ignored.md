# 515 — Parameters a component kind does not take are ignored without a word

**Status:** in progress
**Type:** bug
**Priority:** low
**Area:** Observability and operations
**Summary:** Most component kinds build whatever `params` their descriptor gives and ignore those they do not know, so a parameter misspelt, set on the wrong component or moved to another is lost without a word; only the kinds that read their parameters as one struct refuse it.

## The problem

A component kind reads its parameters in one of three ways, and only the first refuses a parameter
it does not know:

*   **As one struct**, with `config_of_params`, the struct `#[serde(deny_unknown_fields)]`: a
    parameter it does not know is refused when the component is built
    (`ComponentError::InvalidConfig`), naming it. Seven kinds: `mcp_tool`, `a2a_agent`,
    `ollama_client`, `artifact_tool`, `decision_policy`, `compaction` and `terminal_tool`.
*   **One by one**, each parameter it knows taken from `params` by its name - `params.get(…)`,
    `flag_of_params`, `PromptPosition::of_params` -, and the others never looked at:
    `policy_moderator`, `static_system_prompt_provider`, `sleep_tool` and every
    `*_prompt_provider` that takes a `position` or `changes_only`. `skills` and
    `workspace_prompt_provider` read `position` so and the rest as a struct, which therefore cannot
    refuse what it does not know either.
*   **Not at all**: a kind without parameters ignores its descriptor
    (`|_descriptor, context|`). About twenty kinds are written that way, among them
    `summary_compaction`, `notes`, `channel_history`, `create_plan_tool`, `stop_tool`,
    `ask_user_tool`, `help_command` and the moderator policies.

Issue [413](413-compaction-restores-unused-skills.md) made the difference visible: `keep` moved
from `summary_compaction` to `compaction`. Until then, a `keep` set on `summary_compaction` was
read; since, it is ignored, and the agent keeps the default share of its context without anyone
being told. A `max_agent_hop` on a `policy_moderator`, or a `postion` on a prompt provider, is
ignored the same way.

## Why it matters

*   A mistake in a personality - a parameter misspelt, set on the wrong component, or one a
    change moved elsewhere - has no effect and no message; whoever configured it believes it
    holds.
*   Kinds answer the same mistake differently, depending only on how they happen to read their
    parameters today. A kind that gains its first parameter as a struct starts refusing what it
    ignored before.
*   The rule "stored data is not kept compatible, the summary of a change says what to delete"
    relies on a stored configuration that no longer fits failing loudly.

## Considerations

*   **The registry checks, for every kind.** A kind declares what it takes - a configuration type
    with its kind (`ComponentKind`), or that it takes none -, and the registry refuses unknown
    parameters before the factory runs, in one place. The factories of kinds without parameters
    stay as they are; those that read their parameters one by one read them as one struct. A change
    to `ComponentKind` and `ComponentRegistry` in the core.
*   **Every kind reads its parameters as one struct**, `deny_unknown_fields`, a kind without
    parameters a struct without fields - as the `Backends` do with `NoParams`. Small, but the same
    lines in twenty factories, and a new kind that forgets them is back to ignoring.
*   **The registry hands the factory its configuration typed**: no struct named twice, but a
    second `register` and `register_async` for kinds with parameters - four ways of registering
    that differ in one respect.
*   **As it is.** Leaves the mistake silent.

Decided: the first - one rule in one place. The registry checks while it plans a configuration
(`ComponentRegistry::plan`), which both a container and `require_acceptable` do: a parameter is
refused before anything is built, and before a configuration that names it is written - also one
of a component a `ref` brings, or of one that builds on a default, with the default's params
merged. Three steps: first the kinds that read their parameters one by one read them as one
struct; then the declaration and the check; last the `Backends`, which check by `NoParams` in every
factory, follow the same rule.

`examples/seed.yaml` and `examples/config.yaml` give no kind a parameter it does not take. A stored
personality, bundle or session that does no longer builds.

## Done when

*   A parameter that a component kind does not take is refused when the component is built, naming
    the component and the parameter, whether the kind takes parameters or not; a test covers a kind
    without parameters, and one with parameters read until now one by one.
*   The `Backends` refuse a parameter by the same rule.

## History

- **2026-10-30** — Opened after issue 413, which moved `keep` from `summary_compaction` to
  `compaction`: a `keep` still set on `summary_compaction` is ignored since.
- **2026-10-30** — The problem corrected: the kinds that read their parameters one by one ignore
  those they do not know as well. Decided: the registry checks for every kind, the `Backends`
  follow in a step of their own.
- **2026-10-30** — Step 1: `policy_moderator`, `static_system_prompt_provider`, `sleep_tool`, the
  prompt providers, `skills` and `workspace_prompt_provider` read their parameters as one struct
  with `config_of_params` (`PositionConfig` and `ChannelPartConfig` for the prompt providers that
  share theirs); `flag_of_params`, `PromptPosition::of_params` and `of_params_or` are gone.
  `config_of_params` names the parameter that is not what it should be (`serde_path_to_error`), as
  the readers it replaces did, and refuses `params` that are no object: a list would be read by the
  order of the fields, past `deny_unknown_fields`. It refuses a parameter that is `null` as well,
  whether its field may be left out (`Option`) or not, so that `null` is no way of leaving one out
  for some kinds and a mistake for others.
