# 621 — A delegation is announced to nobody

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** Whom a thread was delegated to - its sub-agent - is set in the channel's state after the thread opened, and no event says so, so the frontends guess it: the terminal reads the threads anew when the delegate gives its first brief, the web UI only when something else makes it fetch the channel again; both may name a thread without its sub-agent, or by one that left.

## The problem

A sub-agent's thread (issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md)) is opened first
and delegated into after:

- `ActorChange::Open` records the thread with no delegation and announces `ThreadOpened`, which
  carries the thread and where it hangs (`caelumora-kernel/src/channel/run.rs`).
- `ChannelLoop::delegate` sends `ActorChange::Delegate`, which sets the delegation - giver,
  recipient, briefs - in the channel's state and starts the thread's delegate
  (`hand_to_delegate`, `caelumora-kernel/src/channel/delegation.rs`). Nothing is announced.
- The delegate then gives the recipient the brief to read (`GivenToRead`, sent by
  `ActorId::Delegate` of the thread, in the thread), which the users are shown.

A snapshot names the recipient of each thread (`ThreadSnapshot::recipient`), so whoever reads the
channel after the delegation knows it. A frontend that keeps the threads from the stream does not
learn when to read:

- **The terminal** (issue [615](615-the-full-screen-terminal-lists-no-channels-or-threads.md),
  `Followed::changes_threads`) reads the threads anew at `ThreadOpened`, at `ThreadClosed`, and at
  the first brief the delegate gives - inferring the delegation from it.
- **The web UI** fetches the channel again at `ThreadOpened` and where someone joins or leaves a
  thread (`ChatView.tsx`), not at a delegation; it learns the recipient at whatever fetch comes
  next.

## Why it matters

- The two frontends keep the same thing by different rules, both working around one gap of the
  kernel.
- The terminal's inference misses a delegation whose brief never reaches the thread - the
  recipient left before the delegate carried it, or the channel rejected the brief: the thread is
  named `Thread` instead of by its sub-agent until it is read anew for another reason.
- Where the recipient leaves a thread that stays open, nothing tells the terminal; it keeps naming
  the thread by an agent no longer there.
- Rare cases each, and only a name shown is wrong; nothing is lost.

## Considerations

- **`ThreadOpened` carries the recipient.** Not possible as things are: the delegation does not
  exist yet when the thread opens.
- **An event of its own for a delegation** - say `ThreadDelegated { thread, recipient }`, announced
  in the line as `ThreadOpened` is, and recorded in the history like it -, which both frontends
  follow. It needs a domain event, its history entry, its REST type and the web UI's handling; what
  it says is already in the snapshot.
- **Leave it.** The terminal's inference holds in every case observed; the web UI shows the name
  at its next fetch. Document the rare cases and keep them.
- Whether a recipient leaving a thread needs telling as well is the same question: the web UI
  fetches on it already, the terminal does not.

## Done when

- How a frontend learns whom a thread was delegated to is decided, and both frontends follow the
  same rule.

## History

- **2026-11-27** — Opened from the review of issue 615, step 1: the terminal infers a delegation
  from the delegate's first brief, the web UI does not learn it until it fetches the channel again.
