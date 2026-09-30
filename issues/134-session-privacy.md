# 134 — Reading across sessions is a matter of consent, not of classification

**Status:** open
**Area:** Sessions and channels

## The problem

With the classification model of [133](133-classification-model.md), the
session is no longer a confidentiality boundary: whether a channel may read
another is decided by dominance of their classifications alone, so two
`internal` channels of different sessions may read each other's notes and
history. Today the `session` level keeps that from happening, but for the
wrong reason: what stops one session from reading another is not that its
contents are more sensitive, but that the participants of the one never
agreed to be read by the other.

## Why it matters

- A session is a conversation among its participants. That an agent in
  another session, with other users, can read along is a matter of the
  participants' privacy, whatever the classification says.
- Knowledge beyond the session ([18](18-knowledge-sharing.md)) needs
  exactly this distinction: the rules of confidentiality decide what *may*
  be shared by its sensitivity, the rules of privacy decide who *agreed*.

## Considerations

- **A session may be read by another only with consent** - of its owner, or
  of its participants. How consent is expressed is open: a session-level
  setting ("readable by my other sessions" / "by nobody"), an explicit
  grant to a named session or agent, or membership of the same user or
  group in both.
- **Where it is checked:** the same `ActionType::ReadChannel` question,
  answered by the policy from both channels' sessions in addition to their
  classifications - not a second question, so that every read across
  channels stays one place (issue 71).
- **The default** decides how much of today's behaviour survives step 1 of
  [133](133-classification-model.md): "within the same session only" keeps
  it exactly; anything wider is a change.
- The same question arises for a user reading a channel they are not a
  participant of ([125](125-invisible-admin.md)) and for the agent's own
  memory across sessions ([19](19-memory-tiers.md)).

## Done when

- It is decided how the participants of a session consent to being read
  from elsewhere, and reading across sessions is refused without it.

## History

- **2026-09-30** — Split off from the discussion of
  [133](133-classification-model.md) at the maintainer's request: sessions
  stop being a confidentiality boundary, and what keeps them apart is to be
  modelled as privacy instead.
