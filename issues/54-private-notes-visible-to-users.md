# 54 — Private notes are visible to users

**Status:** done
**Area:** Context & knowledge

## The problem

Private notes ([51](51-agent-private-notes.md)) are meant for the agent
alone, recalled only where their confidentiality allows. Users can read them
anyway, in two places:

- **The context panel** ([41](41-show-message-context.md)) shows what an
  agent's model saw, prompt parts included ([47](47-prompt-parts-with-provenance.md)),
  so every private note the agent was given appears there as the part
  "[Your private notes ...]", together with the channels they came from.
  Opening the panel takes `ReadAgentContext`, which the ownership policy
  grants the agent's owner only; but the owner is usually also a user of the
  channel.
- **The chat itself**: the tool call `note_private` is part of the agent's
  message, with its arguments. The web UI shows tool calls and their
  arguments in the conversation, so everyone who can read the channel sees
  the text of the note as it is written.

## Why it matters

- "No one else sees them" is what the tool tells the model and what the
  design promises. If the text is shown to users anyway, the agent is misled
  about what it can keep to itself, and the confidentiality rule for
  recalling notes in other channels ([19](19-memory-tiers.md), principle 12)
  is undermined by simply opening the panel in the channel where they are
  recalled.
- With several users in a session, the owner reading an agent's private
  notes may or may not be intended; a user who is not the owner reading the
  tool call in the chat is certainly not.

## Considerations

- **The audit purpose of the context panel** ([06](06-context-in-message-for-auditability.md))
  argues against hiding anything from it: it answers "what exactly did the
  model see". Options:
  - Keep private notes visible to the owner, as the one responsible for the
    agent, and say so in the tool description instead of "no one else sees
    them". Honest, simplest; private then means "private from other agents
    and other users", not from the owner.
  - Redact the private notes part in the panel by default - show that a part
    was there, its digest and the channels it came from, not the text - and
    reveal it only under a separate permission (e.g. a new
    `ActionType::ReadPrivateNotes`). The digest keeps the audit trail
    verifiable without disclosing the content.
  - Show them only in the channel they were written in, where every user
    could see the tool call anyway, and redact them where they were recalled
    from another channel.
- **Tool calls in the chat**: the arguments of `note_private` (and the
  result) should not reach users who may not read the note. Options: mark
  tools whose arguments are private (a flag on `Tool`) and have the history,
  REST and the web UI show only that the tool was called; or keep the call
  out of the history users read altogether. The agent's own context still
  needs it.
- The same questions will come back for any tool that handles something
  confidential, so a general mechanism (private tool arguments, redactable
  prompt parts) is preferable to special cases for private notes.
- Related: whether agents count as readers at all is
  [12](12-user-and-agent-permissions.md); an agent recalling a private note
  still passes it on to the channel it answers in (see the history of
  [51](51-agent-private-notes.md)).

## Done when

- It is decided who may see private notes, and the context panel follows
  that decision.
- The text of a private note does not appear in the chat for users who may
  not see it.
- The tool description of `note_private` says truthfully who can see a
  private note.

## History

- **2026-09-25** — Issue created at the maintainer's request after private
  notes were implemented: they appear in the context panel. The tool call
  `note_private` showing the note's text in the chat was added while writing
  it up.
- **2026-09-26** — Decided after the maintainer proposed a privacy setting
  per tool:
  - **Visibility per tool, persisted per call.** `Tool` gets
    `visibility: ToolVisibility { Public, CallOnly, Hidden }` (default
    `Public`), declared by the `ToolProvider` like `requires_approval`.
    `Public`: others see the call, its arguments and its result. `CallOnly`:
    others see only that the tool was called. `Hidden`: others see nothing.
    The maintainer's names were Public / Protected / Private; "Protected" was
    replaced because it says nothing about *what* stays visible and means
    something else in OOP. A level "arguments visible, result hidden" is
    conceivable but no tool needs it yet; the enum can grow.
  - The visibility is stored on `ToolCall` and `ToolCallResult`, not on the
    message: one message can hold calls of different visibility, and the
    result lives in a message of its own that must be filterable without
    looking the call up. What counts is what applied at the time of the call,
    even if the tool changes or its component disappears later.
  - **"Others" are users.** Other agents already see no tool calls of other
    agents (`remove_tools` in the moderator).
  - **Redacted at the source, in the agent.** The agent sends a redacted copy
    in its events (`PartialMessageGenerated`, `CompleteMessageGenerated` and
    the intermediate variants - partials already carry arguments while
    streaming) and keeps the full call in its `MessageContext`. So the
    history, REST, SSE and the moderator need no knowledge and cannot leak;
    redacting only in the web UI would leave the text in the API. The history
    is the public view, the context panel the agent's own view guarded by
    `ReadAgentContext`. History digests (`KnowledgeRef::History`) then refer
    to the redacted version, consistent with what `read_history` shows other
    channels. A tool message with only hidden results is left out of the
    history entirely.
  - The audience is not a second dimension of the enum ("visible to the
    owner only"); it follows from the place: history for all users, context
    for whoever may `ReadAgentContext`.
  - **Approval wins.** A tool with `requires_approval` still shows its
    arguments to the approvers, who cannot decide otherwise; the history stays
    redacted. Forbidding the combination was the alternative.
  - Overriding the visibility per component (e.g. an admin marking MCP tools
    `Hidden`) is possible later via the descriptor, not now.
  - `note_private` becomes `Hidden` (`CallOnly` would be more honest about
    the agent keeping something to itself; a matter of taste).
  - **Context panel: option 1.** It stays the owner's audit view and shows
    the private notes prompt part; the tool description of `note_private`
    says truthfully that no one but the agent's owner can see private notes.
  - Still open by design: an agent repeating a private note in its answer is
    not prevented by any tool setting (see [51](51-agent-private-notes.md)).
- **2026-09-27** — Implemented as decided. `ToolVisibility` on `Tool`,
  `ToolCall` and `ToolCallResult` (stored only where not `Public`); the
  agent stamps every call in `handle_llm_turn` from its tool's declaration
  (`AgentActor::stamp_visibility`, unknown tools are public), results take
  it over from the call. Redaction happens in one place:
  `TurnContext::send_event` sends `ActorEvent::published`, which applies
  `Message::published` to every message an event carries - `CallOnly`
  arguments become `null` and results empty, `Hidden` calls and results are
  dropped, a message with nothing left is not sent; approval requests stay
  unredacted. `note_private` and `revoke_private_note` are `Hidden`; their
  tool description and prompt header now say that no other participant sees
  private notes, only the person responsible for the agent. The web UI shows
  a badge on non-public calls (visible to the owner in the context panel)
  and "Arguments/Result not shown" for `CallOnly` calls. Verified in the
  browser: the call is absent from the chat, the history and the event
  stream, and shown in full with its badge in the context panel.
  The same check found the model's **reasoning** of the message that made
  the hidden call quoting the note in the history (the web UI does not show
  reasoning, the API delivers it). That is split off as
  [61](61-reasoning-leaves-the-agent.md), since who may see reasoning is a
  decision of its own and streaming makes a per-message rule too late. This
  issue's decision - tool visibility and the context panel - is done.
