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
- **2026-09-30** — Decided with the maintainer:
  - Every session has exactly one privacy policy, chosen by a serializable
    `PrivacyConfig` stored with it, as the `security` section chooses a
    `SecurityManager`: `Private` (the default - reading only within the
    session), `SameOwner` (sessions of the same owner) and `SharedAudience`
    (every human participant of the reader also takes part in the source).
    Each is an implementation of a `PrivacyPolicy` trait, so that further
    ones can be added; a closed enum, no registry.
  - A policy must be transitive - what it lets pass on may be passed on by
    the next session's policy without breaking its promise. `SameOwner` and
    `SharedAudience` are, also mixed; a grant to a named session would not
    be.
  - It is asked as part of `ActionType::ReadChannel`, only across
    sessions, and it is the source session's policy that decides. The
    exception that lets `public` entries cross sessions goes: privacy holds
    whatever the classification.
  - A memory entry keeps the policy and audience of its source session from
    when it was remembered; recalling it needs both that and the session's
    current policy to agree.
  - The policy is set when the session is created (also from its template)
    and changed only by its owner; it is shown to all participants.
  - A participant added later sees what was read: [143](143-joining-a-channel-with-foreign-knowledge.md).
  - Reading across sessions is traceable from the start: a `KnowledgeRef`
    names the channel with its session (a `ChannelRef` instead of a
    `ChannelId`), and what an agent recalls from its memory is recorded
    once per origin of the recalled entries, not as one reference to the
    whole memory chain in the channel it was read in. [143](143-joining-a-channel-with-foreign-knowledge.md)
    builds on it.
