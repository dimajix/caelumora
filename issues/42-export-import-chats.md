# 42 — Chats cannot be exported or imported

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** Chats cannot be exported or imported, so a conversation cannot leave the server it happened on or be brought into another.

## The problem

A conversation lives only in the store of the server it happened on. There is no way to take it
out - to keep it, share it, attach it to a bug report, or look at it outside the application - and
no way to bring one in, for instance to continue it on another server or to reproduce what an agent
did with the same history.

The feature: export a chat as a JSON document, and import such a document as a new chat.

## Why it matters

- **Reproducing behaviour.** When an agent acts oddly, the history - and since
  [06](06-context-in-message-for-auditability.md) what the agent saw - is what someone else needs to
  look at the problem or replay it.
- **Keeping and moving work.** Stores are per deployment and, during development, get wiped; a
  conversation worth keeping should survive that.
- **An audit trail outside the system** (overlaps with [05](05-postgres-event-recorder.md)): an
  exported chat is a record that does not depend on the server staying unchanged.

## Considerations

- **What "a chat" is.** At least a channel's history. Possibly the whole session: its channels,
  participants, the template it came from. And possibly what the agents saw - their contexts and
  system prompts (issue 06) - which makes an export useful for reproducing behaviour but large and
  more sensitive. Probably a choice at export time, or levels.
- **A versioned format.** The document needs a format version from the first release, so that later
  versions can still read older exports. It should be the export's own format, not the store's
  internal one, so that store changes do not break exports.
- **Digests.** Messages, contexts and history carry recorded digests (issue 06); an export keeps
  them as recorded, and an import must not recompute them. Whether an imported chain is marked as
  imported, and how its references relate to a context an agent builds after the import, needs a
  decision.
- **Identity on import.** Ids in the document may already exist on the target server; an import
  creates a new session with new ids, or keeps message ids for traceability while the session gets a
  new one. The importing user becomes the owner ([11](11-agent-ownership.md)); other users named in
  the document may not exist there. Agents need a personality - carried in the document, matched by
  name, or chosen at import.
- **Continuing an imported chat.** Imported history becomes the channel's history; whether agents
  also get their imported contexts back, so that they continue as if nothing happened, or start from
  the history, is part of the decision above.
- **Permissions.** Exporting is reading everything the chat contains, including what only the
  agent's owner may see (`ReadAgentContext`); an export must not become a way around that. Importing
  creates entities and goes through the same checks as creating them.
- **Sensitivity.** An export may contain tool output and secrets-adjacent material; it leaves the
  server's access control once downloaded, which the UI should make clear.
- **Where:** REST endpoints (e.g. `GET /sessions/{id}/export`, `POST /sessions/import`) and actions
  in the web UI's session menu.

## Done when

- A chat can be exported as a versioned JSON document and imported as a new chat on the same or
  another server.
- What an export contains, and who may export it, is defined and enforced.

## History

- **2026-09-25** — Opened at the maintainer's request; only recorded for now.
