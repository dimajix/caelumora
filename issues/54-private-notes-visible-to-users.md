# 54 — Private notes are visible to users

**Status:** open
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
