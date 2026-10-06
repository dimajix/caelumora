# 242 — The web UI offers to ask an external agent for another answer, which it never gives

**Status:** open
**Area:** User interaction

## The problem

An external agent rejects every request to answer again (`RetryRefused::Elsewhere`,
[234](234-modify-revoke-and-retry-messages.md)): its remote side keeps what it was given. Its answer
under way cannot be asked for again either - it has no turn here (`Actor::turn_under_way` is
`None`) - so such a request is refused at once with `409 not_under_way`
([235](235-retry-of-an-answer-under-way.md)). The web UI cannot tell an external agent from one run
here - a member's `kind` is `agent` for both - and offers the retry icon on its last message and,
since its `ResponseStarted` carries the id of its work, on its answer under way. The first ends in
the notice "did not answer again", the second in an error.

## Why it matters

An offer that can only fail is noise, and the error of the second reads like a fault of the
server. 234 accepted the first as "an offer, not a promise"; with a second, louder one it is worth
deciding.

## Considerations

- A member could say whether it can be asked to answer again at all - false for an agent of an
  external personality (`PersonalityKind::External`) - beside `can_retry` of the channel, which is
  the viewer's permission. The UI offers the retry icons only where both hold.
- Alternatively the kind of member tells it: `ParticipantKind` could tell an external agent from
  one run here. More general, but a new kind touches every client that matches on it.
- Whether an external agent's work should carry a turn id in `ResponseStarted` at all: it groups
  its messages (issue 241's record) and costs nothing; dropping it would hide the offer but lose
  that.

## Done when

The web UI offers to ask for another answer only of members that can give one.

## History

- **2026-10-06** — Opened while planning [240](240-answers-under-way-not-in-snapshot.md), which
  leaves external agents out of the answers under way for the same reason.
