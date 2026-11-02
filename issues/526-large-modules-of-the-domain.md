# 526 — Several modules of the domain hold many concepts in one file

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Several files of `caelumora-domain` hold several concepts each in up to 1400 lines of code - the actors' events, envelopes and states in `actor.rs`, the history's entries, records and readers in `history/mod.rs`, an agent's context, its model's profile and its resolution in `context.rs` -, so that no first sentence of their module docs can name one responsibility.

## The problem

AGENTS.md asks that every module can be described in one sentence, and CODING-GUIDELINES.md that
the first sentence of its docs names its responsibility. In `caelumora-domain`, several files hold
concepts that a reader looks for apart - and two of the largest have no module docs at all. Lines
counted on 2026-11-02, after issue [523](523-the-model-s-store-module-is-named-after-the-stores.md);
code is what comes before the tests.

| File | Lines (code) | What it holds |
|---|---|---|
| `actor.rs` | 1931 (1406) | `ActorId`; `ActorEvent` with `RequestReason`, `RetryTarget`, `Rejection`, `Pick`; `Envelope` and `EventContext`; `ActorState`, `ProcessingStateHolder`, `ActorDescription`. No module docs. |
| `history/mod.rs` | 2178 (1295) | `HistoryEntry`; `Recorded`, `Audience`, `Recipients`; `Heard`, `Reader`; `History`, `fold`, `HistoryPage`. |
| `context.rs` | 1870 (934) | the references of what a model is given (`PromptRef`, `ContextItem`, `KnowledgeRef`, `KnowledgeSource`, `ContextRef`, `FrameRef`, `PromptPart`); how a model is given it (`ModelProfile`, `ReasoningHistory`, `SystemMessages`, `SenderFormat`, `SenderStyle`); resolving them (`ContextResolver`, `ResolvedContext`). |
| `tool.rs` | 770 (642) | a tool as offered (`Tool`, `ToolSet`, `ToolParameters`, `ArgumentLanguage`, `ToolVisibility`, `ToolPresentation`); a call as recorded (`ToolCall`, `ToolCallResult`, `ToolResultStatus`, `ToolMedia`, `ToolCallState`, `ToolCallStatus`). |
| `approval/mod.rs` | 1230 (617) | asking (`Approver`, `ApprovalStrategy`, `ApprovalRequest`, `ApprovalResponse`, `ApprovalDecision`, `ApprovalStatus`); the rules (`ApprovalRule`, `RuleDecision`, `Reach`, `ToolMatcher`, `ArgumentMatcher`, `Coverage`, `RuleError`, their limits). |
| `confidentiality.rs` | 1027 (605) | the classification (`Confidentiality`, `Compartment`, `Classification`, `ClassificationError`, `LabelCauses`); what is configured and who is cleared (`Compartments`, `Clearance`, `Clearances`); the question of a copy (`Place`, `Reading`); the second axis (`Visibility`). |
| `channel.rs` | 814 (611) | the channel's events; `Channel` and `ChannelField`; its history's access (`HistoryAccess`, `HistoryStart`); its state (`ChannelState`, `ChannelStateChange`); its participants (`Participant`, `InternalActor`, `FormerParticipant`). |
| `query.rs` | 562 (393) | what a condition names (`Field`, `FieldPath`, `NoField`, `FieldValue`); the condition (`Filter`); the query and its page (`Query`, `Cursor`, `Page`). |

## Proposal

Each becomes a directory whose `mod.rs` holds the module's docs, what its parts share, and the
re-exports, so that every public path stays as it is (`caelumora_domain::actor::Envelope`,
`caelumora_domain::confidentiality::Classification`) and no file outside the module changes. The
tests go with what they test.

| Module | Files |
|---|---|
| `actor/` | `mod.rs` (`ActorId`), `event.rs` (`ActorEvent` and its parts), `envelope.rs` (`Envelope`, `EventContext`), `state.rs` (`ActorState`, `ProcessingStateHolder`, `ActorDescription`) |
| `history/` | `mod.rs` (`History`, `fold`, `HistoryPage`), `entry.rs` (`HistoryEntry`), `recorded.rs` (`Recorded`, `Audience`, `Recipients`), `reader.rs` (`Reader`, `Heard`), beside `filter.rs` |
| `context/` | `mod.rs`, `item.rs` (the references), `profile.rs` (`ModelProfile` and its settings), `resolver.rs` (`ContextResolver`, `ResolvedContext`) |
| `tool/` | `mod.rs` (`Tool`, `ToolSet` and how they are offered), `call.rs` (a call and its result as recorded) |
| `approval/` | `mod.rs` (asking), `rule.rs` (the rules), beside `shell.rs` |
| `confidentiality/` | `mod.rs`, `classification.rs`, `clearance.rs`, `reading.rs`, `visibility.rs`, `testing.rs` |
| `channel/` | `mod.rs` (`Channel`, `ChannelField`, its events), `history_access.rs`, `state.rs`, `participant.rs` |
| `query/` | `mod.rs` (`Query`, `Cursor`, `Page`), `field.rs`, `filter.rs` |

Which files a module is cut into is decided with its step, from its code: the table names the
concepts each file is to hold, not yet every item.

## Considerations

- **Left as they are.** `agent/channel_state.rs` (809 lines of code) is one type, whose methods
  would be spread over files without a concept of their own; `chain.rs`, `message.rs` and
  `digest.rs` each hold one concept; `snapshot.rs` (16 types) and `config.rs` (16 types) are many
  small types of one kind each - snapshots of a viewer, sections of the configuration - read
  together; `template.rs` is one concept, the template of a session and its parts.
- **Module docs.** `actor.rs`, `channel.rs`, `config.rs` and `event.rs` begin without `//!`; the
  modules cut get them in their step, with a first sentence naming their responsibility.
- **Grouping the flat modules.** Beyond the large files, the crate root has some forty modules
  side by side; some belong to one subject - what an agent is given and records (`context`,
  `transcript`, `turn`, `work`, `plan`, `ledger`), beside the existing `agent/`. Grouping them
  changes public paths throughout the workspace; it is a question of its own and not part of this
  issue.
- **Order.** One commit per module, each without behaviour change, so that each is reviewed on
  its own; `query` and `confidentiality` first, as already discussed, then by size. Depends on
  [523](523-the-model-s-store-module-is-named-after-the-stores.md) being merged, which made
  `query` a module of its own.

## Done when

- The modules of the table are directories, each file of them describable in one sentence, with
  module docs whose first sentence names its responsibility.
- No public path of the domain has changed, and the tests, clippy and the pre-commit hook pass.

## History

- **2026-11-02** — Opened at the maintainer's request, after splitting `query` and
  `confidentiality` was proposed in the work on issue
  [523](523-the-model-s-store-module-is-named-after-the-stores.md).
