# 238 — Only an agent's last answer can be asked for again

**Status:** open
**Area:** Sessions & channels

## The problem

An agent answers again only instead of its last answer ([234](234-modify-revoke-and-retry-messages.md)):
once anything was said or done after it, a request is rejected (`RetryRefused::NotLast`). An answer
further up the conversation cannot be replaced, and neither can the alternatives of one be kept and
compared - each retry takes the previous answer back.

## Why it matters

Comparable chat systems let one go back to an earlier point and continue from there, and keep the
alternatives to switch between (`< 2/3 >`). It is the way out of a conversation that went wrong
several messages ago.

## Considerations

- A new answer cannot be placed above what answered the old one without forking: the messages after
  it were said in reply to the old version. This is branching, which
  [08](08-retry-and-edit-past-answers.md) planned in detail - a tree of conversations every store
  of a channel follows - and rejected for 234 as more than was asked for. Its plan and three
  revisions, including the open question of what a switch means for an external agent, are the
  starting point.
- The chains are trees with a head already (08, step 1), used by given-up turns
  ([145](145-ended-turns-leave-a-record.md)) and external agents
  ([205](205-external-agent-message-context.md)).
- A cheaper variant without forking: take back everything below the answer, then retry. It keeps no
  alternatives and destroys what was said after, for everyone.

## Done when

It is decided whether earlier answers can be replaced - by branching, or by taking back what
follows - and what that means for the other participants of a channel.

## History

- **2026-10-06** — Opened from open question 10 of [234](234-modify-revoke-and-retry-messages.md);
  the issues that referred to 08 for branching (among them [19](19-memory-tiers.md), phase 3) now
  have this one for what is left of it.
