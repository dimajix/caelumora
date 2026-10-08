# 264 — An agent reads channels of its session that its owner may not read

**Status:** open
**Area:** Identity and access management

## The problem

A user reads a channel of their session if they may read it (`GetChannel`, by ownership: the
channel's owner and its participants) and are cleared for its classification. An agent reads
another channel of its session by a different rule: by whether the source's classification flows
into the classification of the channel it reads from (`ReadChannel`, issues
[17](17-information-sharing-within-session.md), [133](133-classification-model.md)) - not by who
takes part in the source, and not by whether its owner may read it.
`SessionView::readable` (`caelumora-runtime/src/view/session.rs`) makes the two rules explicit:
a viewer with a place - an agent - is asked `may_read_from`, a viewer without one `may_read_channel`
and its clearance.

Since issue [255](255-whose-name-agents-act-in.md) an agent acts with its owner's rights in all it
does - and the policy is asked in a context naming the owner as principal. But `ReadChannel` does
not look at the principal under the ownership policy, so for another channel of the same session
the decision does not use the owner's rights at all. The security suite shows it
(`an_agent_reads_another_channel_with_its_owners_rights_whoever_asked`): alice's channel "side",
which bob may not read, is read by alice's agent when bob asks - which 255 accepts, since it reads
with alice's rights. But bob's agent in the same channel would read "side" as well, though bob,
its owner, may not read it.

## Why it matters

- **Membership is no boundary for agents.** A channel of a session that only some of its people
  take part in keeps its conversation from the others - but not from their agents, if its
  classification flows into a channel they share. Whoever asks such an agent learns it.
- **It contradicts 255's sentence** that an agent reads with its owner's rights: within a session,
  it reads with more.
- **The rule was chosen on purpose.** Issue 133 decided that `ReadChannel` depends on the
  channels, not on the reader, "so that the answer does not change when someone joins or leaves
  one" - what an agent remembered, and what was released to a channel, should stay readable
  however membership changes. History 12 (2026-09-25) records the stricter rule before it: every
  user of the reading channel had to be allowed to read the source.

## Considerations

- **Keep the rule, and say so.** Within a session the boundary is classification, not membership:
  who wants a channel kept from others in the session gives it a compartment the others' channels
  do not read. Cheapest; documents what is. Needs SECURITY.md and CONFIDENTIALITY.md to say that
  membership does not hide a channel from the session's agents, and perhaps a way to make a
  channel's compartment easy to set.
- **Ask the owner's rights as well.** An agent reads a channel as a whole only if its principal
  may read it (`GetChannel` of the source, in the turn's context) besides the flow rule;
  entries released to its channel stay readable regardless. Consistent with 255; the answer then
  changes when the owner joins or leaves the source, which is what 133 avoided - and it would have
  to be decided whether that also holds for what the agent remembered there (issue 131) and for
  its private notes (issue 51).
- **Ask the readers of the reading channel** - every user of it may read the source, the rule of
  12's history. Protects the asker's side rather than the owner's; changes with membership on
  both sides.
- **A policy decides.** `ReadChannel` already carries the context with principal and initiator;
  a policy other than the ownership one may look at them. The ownership policy would still need
  one of the answers above.

Related: [255](255-whose-name-agents-act-in.md) (an agent acts with its owner's rights),
[133](133-classification-model.md) (the flow rule), [17](17-information-sharing-within-session.md)
(reading within a session), [131](131-agent-memory-bypasses-policy.md) (memory recall asks
`ReadChannel`), [51](51-agent-private-notes.md) (private notes across channels),
[19](19-memory-tiers.md) (point 11, reading across channels).

## Done when

- It is decided whether membership of a channel limits what agents of its session read, and the
  decision is in SECURITY.md and CONFIDENTIALITY.md.
- If it does, the policy decides it, and the security suite pins an agent whose owner may not read
  a channel.

## History

- **2026-10-08** — Opened at the maintainer's request, found while closing
  [255](255-whose-name-agents-act-in.md) (step 3): reading another channel of the same session
  asks only `ReadChannel`, so whose rights an agent reads with does not decide it.
