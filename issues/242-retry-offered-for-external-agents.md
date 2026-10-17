# 242 — The web UI offers to ask an external agent for another answer, which it never gives

**Status:** done
**Area:** User interaction
**Summary:** The web UI offers to ask an external agent for another answer - of a message and while it answers - which it never gives.

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

## Plan

Decided with the maintainer.

- **Whether an agent answers again is a rule of its kind**, in the model:
  `PersonalityKind::can_retry`, false for `External` - its remote side keeps what it was given
  (`RetryRefused::Elsewhere`). The kind stays decided where an actor is made
  (`channel::Member::of`, issue [189](189-agent-kind-decided-twice.md)); the views ask only this
  question of it.
- **`can_retry` moves from the channel to the member**: `MemberSnapshot::can_retry`,
  `MemberView::may_retry` - the viewer's `RetryMessage` in the channel, of an agent taking part
  whose kind answers again. So `can_approve` and `can_read_context` are built: the permission and
  where it makes sense, in one flag. `ChannelSnapshot::can_retry` and `ChannelView::may_retry` go;
  the REST API and the web UI follow (`ChannelResponse::can_retry` goes,
  `ChannelMemberResponse::can_retry` comes).
- **`AgentMemberState` keeps the personality** it reads, not only its introduction: the
  introduction and the kind share one read, made when either is first asked for. An agent whose
  personality is gone is not run (189) and does not answer again. A snapshot now reads the
  personality of every agent member; a cache across requests stays with
  [182](182-entity-reads-not-cached.md).
- **The web UI** offers both retry icons by the member's `can_retry`: of the last message by its
  sender, of the answer under way by its answerer.
- Not changed: the server still refuses (`elsewhere`, `409 not_under_way`) for a client that asks
  anyway; the TUI's `/retry` is typed, and its refusal shown.

## Done when

The web UI offers to ask for another answer only of members that can give one.

## History

- **2026-10-06** — Opened while planning [240](240-answers-under-way-not-in-snapshot.md), which
  leaves external agents out of the answers under way for the same reason.
- **2026-10-06** — Planned with the maintainer: the member says whether the viewer may ask it to
  answer again (`can_retry` moves from the channel to the member), by a rule of the personality's
  kind; the member view keeps the whole personality it reads.
- **2026-10-06** — Done, as planned. `PersonalityKind::can_retry` (named after the established term
  on the maintainer's word) is false for `External`. `AgentMemberState` keeps the personality it
  reads (`OnceCell<Option<Personality>>`); the introduction and `MemberState::can_retry` share the
  read, and a gone personality answers no. `MemberView::may_retry` asks `RetryMessage` only of an
  agent taking part that can retry; a kind that cannot be read is not offered.
  `ChannelView::may_retry` and `ChannelSnapshot::can_retry` are gone, `MemberSnapshot::can_retry`
  and `ChannelMemberResponse::can_retry` came. The web UI offers both retry icons by the member's
  `can_retry`. Tests: a kind's rule in the model; an external member is not offered, an agent run
  here is offered to a participant and no other member is (`view::tests`); the security suite's
  `RetryMessage` test asks it of the member. CHANNELS.md (*Answering again*), REST-SERVER.md,
  WEB-UI.md.
