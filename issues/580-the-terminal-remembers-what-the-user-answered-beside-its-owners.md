# 580 — The terminal remembers what the user answered beside the owners of approvals and questions

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** `caelumora-tui` keeps the ids of the approvals and questions the user answered in a set of its own (`Talk::answered`), so as not to ask them again until the channel tells their end - beside `Followed`, which keeps the approvals pending, and the user's actor, which keeps the questions open; an answer the channel rejects stays in it, and its approval or questions are never asked again.

## The problem

The terminal asks what waits for the user of its own accord (issue
[571](571-approvals-and-questions-in-the-terminal.md)). Between the user's answer and the end the
channel shows of it, the approval is still pending in `Followed` - until its own `ApprovalAnswered`
comes back - and the questions still open in the user's actor - until `QuestionClosed`. So that
they are not asked again meanwhile, `Talk` keeps the ids answered in a set beside them
(`Talk::answered`), for the run of the program.

*   **Beside the owners.** Which approvals are pending is `Followed`'s to know, which questions are
    open the actor's ([APPROVALS.md](../docs/APPROVALS.md), "rather than keeping a list beside
    it"); the set is a second list, which the document names as an exception.
*   **A rejected answer.** Where the channel rejects the answer (`EventRejected`, for instance
    `AnswerQuestion` refused), the questions stay open in the actor, but their id stays in the set:
    they are asked no more, and `/answer <id>` asks them only until the next line, without telling
    why. Rare: the actor refuses what cannot count before anything is sent.

## Considerations

*   **`Followed` marks an approval answered as the user's answer is sent**, as it drops it once the
    answer comes back; for questions, the actor - which knows when an answer is sent and when it
    is rejected - tells them as answered, or `Followed` keeps them so beside what it knows of
    them. Then `Talk` asks only what its owners tell is to be decided.
*   **Taking an id out as its answer is rejected**, kept in `Talk` - smaller, but the list stays
    beside its owners.
*   **Documenting the behaviour and holding it by a test** - the least change, the rare case kept.

## Done when

- What the user answered is not asked again until its end, without a list of the terminal's own
  beside the owners - or the list is documented with its reason.
- An answer the channel rejects leaves its approval or questions to be asked again.

## History

**2026-11-14** - Opened from the review of step 2 of issue 571, where the maintainer decided it as
an issue of its own.

**2026-11-16** - Decided by the maintainer for the first consideration, with the second point of
"Done when" changed: an answer that is rejected is not asked again, which is documented with its
reason and held by a test. The rejections an answer meets are standing ones but for one - the
channel admits a user's answer to questions only with `AnswerQuestion`, and a well-formed answer
to an approval always; the approver refuses what it does not hold and the answer of one who may
not approve -, so that asking again would only be rejected again, in the terminal line after
line. The exception: the approver also refuses an answer where it cannot read the agent that
asked, a store failure that may pass; the terminal then waits for the approval to end, where the
web UI offers it again. The maintainer decided that this difference stays, documented at
`Followed::answered_approvals`.
Done: `UserActor::answer_questions` forgets the questions as the answer is sent, as a message
declining them did already, so that `questions_to_answer` tells only those still to be answered;
`Followed` keeps the approvals the user answered - by hand as the answer is sent
(`Followed::approve`), by a rule as the channel shows it - until the approver ends them, also
across `catch_up`; `Talk::answered` is gone, and with it the exception in `docs/APPROVALS.md`.
