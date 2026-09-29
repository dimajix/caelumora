# 108 — `#[serde(default)]` where it only supports stored data that no longer exists

**Status:** done
**Area:** Persistence

## The problem

No legacy data needs to be supported, yet a number of fields carry
`#[serde(default)]` (and tests) only so that documents stored before the
field existed still load: all 15 on `AgentChannelState`
(`caelumora-model/src/agent.rs:175-247`), `AgentState::memory` (`:347`),
`Channel::history_limits` (`skip_serializing_if`, `channel.rs:200`),
`ContextRef::prompts` (`message.rs:329`), `KnowledgeSource::History.start`
(`:312`), `ToolCallState::result_status` (`tool.rs:188`, doc "stored before
it existed"), `ToolCallResult.status` with the test
`stored_results_without_a_status_read_as_ok` (`tool.rs:171-173, 406`),
`SecurityAttributes::contributors` (`security.rs:103`, doc "stored before
owners were recorded"), `Session::commands` with the test "Sessions stored
before…" (`session.rs:177`, `template.rs:320-324`). Comments "formerly in
TemplateStore" (`store.rs:33, 40`) point at a history nobody needs.

## Why it matters

A default hides a missing field. In stored *state* that is a bug detector
switched off: a `turn_messages` that failed to serialise loads as empty
instead of failing.

## Considerations

- Remove the attribute and the legacy tests from stored state
  (`AgentChannelState`, `AgentState`, `ContextRef`, `KnowledgeSource`,
  `ToolCallState`, `ToolCallResult`, `SecurityAttributes`, `Session`).
- **Keep it on hand-written documents**, where leaving a field out is
  ergonomics, not legacy: `Personality::{components, security_attributes,
  reasoning_confidentiality}`, `Channel::confidentiality`,
  `ComponentDescriptor`, `ChannelTemplate`, `AppConfig` - and say so in a
  comment, so the next reader does not remove them.
- Keep it where `skip_serializing_if` is paired (the round trip needs the
  default).

## Done when

Every remaining `#[serde(default)]` on a model type is on a hand-written
document or paired with `skip_serializing_if`, and says which.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, section 6).
- **2026-09-29** — Done. Removed where only stored data of an older shape
  needed it: `AgentChannelState::{processing_state, notes, private_notes,
  prompts, turn}` (the other ten were already gone), `AgentState::memory`,
  `Message::internal`, `ContextRef::prompts`, `ToolCallState::result_status`,
  `Session::commands` with its legacy test, and `Channel::confidentiality` -
  which the considerations counted as hand-written, but a channel document
  is only ever built by code (a request is a `ChannelSpec`). Also
  `Tool::requires_approval`: a tool definition is built by its provider and
  never read from data. Kept, with a comment, on the documents written by
  hand - since issue 105 a seed file holds personalities and templates:
  `Personality`, `ComponentConfig`/`ComponentDescriptor`,
  `SessionTemplate`/`ChannelTemplate` (and the approver's `strategy`),
  `SecurityAttributes::contributors`, and `AppConfig`. Kept where
  `skip_serializing_if` is paired - the attribute itself says why; the
  `ToolCallResult::status` doc and its test no longer speak of old data.
  The `SecurityAttributes` doc no longer explains `ActorId::None` by data
  "stored before owners were recorded". The "formerly in TemplateStore"
  comments were already gone.
