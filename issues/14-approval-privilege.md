# 14 — Who is allowed to approve

**Status:** done
**Area:** Tools & approvals

## The problem

`ApproverActor` counts responses from every `Participant::User` in the channel and resolves the
request by the configured `ApprovalStrategy`. There is no notion of who is *entitled* to answer: any
user in the channel can approve any tool call, and the TODO entry's second half — *"other approvals
must be ignored"* — has nowhere to be enforced.

Since responses arrive via the REST API, and the REST API has no authentication
([10](10-user-authentication.md)), the answer today is effectively "anyone".

## Why it matters

Approval is the system's one human-in-the-loop safety mechanism. A check that anyone present can
satisfy provides the appearance of control rather than control. It matters most in exactly the case
approvals exist for: a multi-participant channel where not everyone is equally trusted.

## Considerations

- `ApprovalStrategy::Majority` and `All` compute against the current user count in the channel
  (`get_user_count`). If only some users are eligible approvers, that denominator is wrong and both
  strategies silently change meaning. Eligibility has to be defined *before* those strategies are
  trustworthy, not after.
- Ignoring an ineligible response is not enough on its own — the responder should be told their
  answer was disregarded, or they will assume it counted.
- Eligibility could be a property of the `Approver`, of the channel's participant list, or derived
  from ownership of the agent whose tool call is being approved ([11](11-agent-ownership.md)). The
  last is appealing: the person responsible for the agent is the person who should answer for it.
- Approval responses are security decisions and belong in the recorded event stream with the
  identity attached ([05](05-postgres-event-recorder.md)).
- `ApproverActor` recomputes pending approvals when participants join or leave; eligibility changes
  have to flow through the same recheck.

## Done when

- Eligibility to approve is explicit and enforced; ineligible responses are rejected rather than
  counted.
- The `Majority`/`All` denominators count eligible approvers only.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-23** — Unblocked by [30](30-user-actor-for-authenticated-user.md): an approval now
  carries the user who made it (their own `UserActor`), and approvals from non-participants are
  already refused. Deciding *which* participants may approve is what is left.
- **2026-10-05** — Decided with the maintainer:
  - **The `SecurityManager` decides, the approver enforces.** A new action `ApproveToolCall { agent,
    channel }` with the question `may_approve`. The approver is where an answer counts, so it is the
    enforcement point: it asks with the context an answer came with (`Enforcement::require`) and
    leaves out answers that are refused. It asks the same question for each user taking part, with
    the context of their own `UserActor`, so that `Majority`, `All` and `DenyOnFirst` count only
    those who may approve. `UserActor::send_approval` does not check as well.
  - **Who may approve:** the agent's owner and its contributors - who is responsible for an agent
    answers for it, also in someone else's channel. The agent's attributes are loaded, never taken
    from the request, which the agent makes itself. For an agent of the system, the channel's owner
    approves.
  - **Everybody sees a pending approval, only who may approve is asked:** the views tell with
    `can_approve`, and the web UI shows the buttons only then. An answer of someone else is
    therefore no misunderstanding but a client going round the UI; it is left out and logged, and
    the sender is not told - which settles the consideration above that the responder should be
    told.
  - **Nobody may approve:** a request nobody in the channel may approve is denied at once instead of
    waiting for its timeout, for every strategy.
- **2026-10-05** — Done as decided. `ApproveToolCall { agent, channel }` with `may_approve`; the
  ownership policy grants it the agent's owner and contributors, for an agent of the system the
  channel's owner. `ApproverActor` enforces it on every answer and counts, for every strategy, only
  the answers of those taking part who may approve now, asked with their `UserActor`'s context; a
  request nobody may approve is denied at once. Members carry `can_approve`, and the web UI shows
  the buttons only then. In the security suite. Not done here: the model is told of such a denial as
  if a user had declined. The approver trusts the context an answer carries, which nothing
  guarantees belongs to its sender: [228](228-envelope-sender-not-enforced.md).
