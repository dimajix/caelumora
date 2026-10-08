# 257 — A facet and its components live in two modules side by side

**Status:** done
**Area:** Code structure

## The problem

Of most facets the trait and the components that provide it live in two sibling modules: `tool`
and `tool_impl`, `llm` and `llm_impl`, `command` and `command_impl`, `moderator` and
`moderator_impl`, in the model `store` and `store_impl`, `secret` and `secret_impl`,
`user_provider` and `user_provider_impl`; the actors and the transport are split the same way.
The split has no rule of its own beyond the suffix:

- `moderator` holds the `Moderator` trait, `moderator_impl` the second facet of a channel's
  moderation, `ResponderPolicy` - a trait among the implementations, which issue
  [53](53-facet-trait-placement.md) had moved out of `component_impl` for the same reason.
- `actor_impl/mod.rs` is no list of implementations but 390 lines of how an agent's actor takes in
  what it is told, next to `actor.rs` with the trait.
- Paths name the suffix rather than the subject (`crate::llm_impl::ollama_client`), and what the
  two modules share must be `pub(crate)`, where a parent module and its children would see it
  anyway.

A sibling `_impl` module is rare in Rust. The common form is one module per subject: the trait in
it, its implementations as its submodules (`std::io`, `tokio::sync`, `serde::de::value`).

## Considerations

- **One module per facet**, as `terminal` already is (issue [251](251-open-terminal.md)): `mod.rs`
  holds the trait, what consumes it and `register_all`; the components are its other files.
  Paths change, behaviour does not; nothing stored names a module.
- **`mod.rs` or `foo.rs` beside `foo/`.** Both are Rust 2018; the code uses `mod.rs` throughout
  (`terminal`, `view`, `controller`, `agent_turn`), so it stays.
- **A `mod.rs` that becomes a dumping ground**: what is more than the facet goes into a submodule
  of its own - the actor's intake.
- **`component_impl` does not fit**: `component` is the component framework, `component_impl` the
  components without one facet to sort them by. Merging the two would mix the framework with
  components.
- **`external_impl`** fits the pattern; decided last.

## Done when

- No module is the `_impl` sibling of another, except `component_impl` - and `external_impl`, if
  it is decided to stay.
- COMPONENTS.md says where a facet and its components live.

## History

- **2026-10-08** — Opened and decided with the maintainer: `llm`, `command`, `actor`, `transport`,
  `moderator` and `tool` in the runtime, `secret`, `store` and `user_provider` in the model are one
  module each, the trait in `mod.rs`. `ResponderPolicy` now lives beside `Moderator`. Of
  `actor_impl/mod.rs`, how an agent's actor takes things in went to `actor::intake`, the helpers
  of answering to `actor/mod.rs` beside `Answering`, and `apply_stage_processors` to
  `crate::message`, beside the stage traits it runs. `component_impl` stays; `external_impl` is
  looked at last. No change in behaviour; nothing stored changes.
- **2026-10-08** — Done: `external_impl` is `external`, as decided with the maintainer - a facet
  like `llm`, so a module of its own beside it rather than one under an `agent` module.
  `external/mod.rs` holds `ExternalAgent` and `register_all`, `a2a_agent/` is its submodule.
  `component_impl` is the one `_impl` module left.
- **2026-10-08** — In the same pass, with the maintainer, the runtime's `agent_` files became one
  module `agent`: `turn` (with `tool_calls`, which only the turn uses), `state`, `notes`, `memory`
  and `work`, which both kinds of agent share. `agent_run_control` is the `AgentActor`'s alone and
  went to `actor::agent::run_control`.
