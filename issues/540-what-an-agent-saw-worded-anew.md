# 540 — What an agent saw is worded anew with the texts of today

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Audit
**Summary:** What an agent saw when it wrote a message is resolved with the catalogue's texts and the resolver's code as they are when it is asked, so it differs from what was sent once a template or the resolver changed, and nothing tells that it does.

## The problem

Issue [06](06-context-in-message-for-auditability.md) made what an agent saw reconstructable: a
message records the `ContextRef` it was produced from, its prompt parts and tool sets kept by their
digests (issue [383](383-prompt-texts-and-tool-sets-kept-per-turn.md)). What the `ContextResolver`
words itself is not kept: the sender's mark and time of each message, the notes on attachments, the
notices of who joined or left and of turns given up. It words them each time it resolves a context,
"with the texts of that variant as they are then" ([AGENT-TURN.md](../docs/AGENT-TURN.md)), and by
its code as it is then.

## Why it matters

- **"What the agent saw" can be wrong.** After a template of the catalogue or the resolver
  changed, an old message is shown with a context it was never given - the very question an audit
  asks it is to answer.
- **Unnoticed.** Nothing records what was sent, so the difference cannot even be seen.

## Considerations

- **The catalogue's texts by their digest.** The texts the resolver words with are kept as the
  prompt texts are, by their digest, and the `FrameRef` names them; an old context is worded with
  the templates of its time.
- **A digest of what was sent.** `produced_from` carries the digest of the request as resolved;
  resolving it again and comparing tells whether the resolver's code changed what it gives - what
  Claude Code's `requestDigest` does.
- **Keeping the requests** themselves, as Claude Code keeps its blobs, is the last resort, should
  differences turn out to occur in practice: it costs a copy of every call.
- Issue [537](537-prompt-parts-placed-anew-every-turn.md) put this after it by decision.

## Done when

- An old context is worded with the texts of its time.
- Where what is resolved now differs from what was sent, that is told.

## History

- **2026-11-06** — Opened from the comparison of Claude Code's session logs with Caelumora's
  concepts that led to issue 537 (its exact reconstruction, and point E there).
