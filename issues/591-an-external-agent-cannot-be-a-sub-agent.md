# 591 — An external agent cannot be a sub-agent

**Status:** in progress
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** The sub-agents of issue 558 are agents of internal personalities alone: an external agent's intake does not pass on what it is given to read, and its delegate would take a task that outlives its request for one that ended without an answer.

## The problem

Issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md) lets an agent hand a task to a sub-agent
in a thread of its own (issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md)): the
kernel's delegate gives the sub-agent the brief to read (`GivenToRead`), asks it, and gives back
what it answers. An external agent ([179](179-external-agents-via-a2a.md)) takes part in a thread
as any agent does, its remote conversation being that thread's, but as a sub-agent two things are
missing:

- **What it is given to read.** Its intake has to take in `GivenToRead` and pass it to its remote
  agent as text.
- **An answer that comes later.** A remote task can outlive its request and is answered when the
  external agent speaks on its own (issue [199](199-external-agent-speak-wakes-all.md)). The
  delegate would take the end of the turn before it as an answer that ended without one.

So the first version of 558 starts agents of internal personalities alone; the sub-agent tool
refuses an external one with an error.

## Considerations

- **Taking in what is given to read**: as text, in the request to the remote agent that follows,
  or as a message of its own to the remote conversation.
- **The delegate and a late answer**: the delegate keeps the brief open while the sub-agent has a
  remote task under way, and takes what the external agent says on its own as the answer to it; or
  the external agent's turn ends only once its task did.

### What would happen with a late answer

Today no external agent is started as a sub-agent: `SubAgentWriter::personality` refuses an external
personality before anything is made (`SubAgentError::External`). What follows is what would happen
if that refusal were lifted and nothing else changed - read in the code on 2026-11-27, not tried,
for the second point of *The problem*:

- A request whose remote task outlives it - the stream ends or breaks before the task does - is
  answered with nothing: `Ending::Answered(None)`, published as a `TurnFinished` without messages
  (`caelumora-agents/src/actor/external_agent.rs`, `exchange`, `answered`). What came so far is
  kept with the task, to be said with the rest when it ends (issue 195).
- The delegate takes the recipient's first turn that ends after the brief was given as its answer
  (`DelegateActor::take_in`), so it gives back "ended without an answer"
  (`kernel.delegation_unanswered`).
- The main agent's word on it is then due (issue 598): a turn of the giver that ends without a
  brief lets the delegation go, the session follower closes the thread, and the external agent
  leaves the channel with it - and leaving cancels every open remote task
  (`ExternalAgentActor::run`, `has_left`). **The late answer is lost, and the remote work is
  cancelled.**
- Inferred, not tried: the task's later end is said with the turn id of the request that began it
  (`speak`, `answered`), as a `TurnFinished` after the brief was given. Had the main agent given a
  next brief meanwhile, the delegate would take the late answer as that brief's.
- A question back from the remote agent already works: it is the request's answer, given back as
  such, and `ask_subagent` continues the task with the next request.

The first point of *The problem* seems done already, read in the code but not covered by a test:
since step 2 of issue 557 (2026-11-18), the external agent's actor lets `GivenToRead` in
(`ExternalAgentActor::event_filter`), takes it in by the intake every agent shares - as a
`TranscriptEntry::Given` (`intake.rs`) -, and `ContextResolver::given_messages` gives the remote
agent the given entries of its transcript, framed as messages are. A test that a brief reaches the
remote agent would settle it.

### Approaches to a late answer

- **A. The request ends only with its task.** The external agent does not answer a request whose
  task is still open; its turn lasts until the task ends, and what it would have said on its own is
  the request's answer. The delegate needs no change. But a cancel or a stop has to reach a turn
  that may last hours, the channel shows the agent busy all that time, and while it answers a
  request what its other tasks send waits (CHANNELS.md *External agents*) - for hours too.
- **B. The delegate knows a late answer.** The end of a request whose task is still open is marked
  as such - a reason on `TurnFinished`, or an event of its own. The delegate then gives back that
  the sub-agent is still working and its answer will follow, keeps the brief open, takes the task's
  later end as its answer, and the giver's word is not due before. The main agent is told what
  happens; it needs the mark on the agent side and a state of the brief in the delegation.

Recommended: B. An external agent's work is long by nature, and the main agent should be told that
it is under way rather than be given an empty answer or wait in silence.

## Decision

B, as the maintainer decided on 2026-11-28: the end of a request whose task is still open is marked,
and the delegate tells the main agent that the answer follows, keeps the brief open and takes the
task's later end as its answer; the giver's word is not due before.

The first point of *The problem* is done and tested since issue 557
(`an_external_agent_is_given_what_a_delegate_gives_it_to_read_and_answers`).

### Plan

Built as the channel follows its giver's word (issue 598): the channel follows a recorded
`HistoryEntry` in the delegation (`ChannelState::follow_giver`, `Word`), and the delegate does what
the delegation's state says, so that a restart loses nothing. Each step is reviewed on its own:

1.  **A deferred answer.** `TurnFinished` and its `HistoryEntry` say whether the turn's answer is
    deferred (`deferred`, stored as `Recipients` stores its rare flags): its work goes on beyond it,
    and its sender says the answer on its own, with the same turn id. An external agent defers the
    answer of a request whose stream ends before its open task does; the task's end then always
    ends that turn, with nothing said if nothing came, so that whoever waits for the answer is not
    left waiting.
2.  **The brief deferred.** A brief's stage (`BriefStage`: `Waiting`, `Given`, `Deferred { told }`)
    replaces where it was given. The channel follows the recipient's turn ending deferred
    (`Delegation::follow_deferred`); what the delegate gives back of a brief deferred and not told
    yet tells the giver that the answer follows (`kernel.delegation_deferred`) and leaves the brief
    open, no word due; the turn's later end is the answer.
3.  **A followed task taken up again.** A task whose followed stream ends or breaks before it does,
    or that cannot be followed, is followed again after a delay that grows, not only at the actor's
    next run; one its remote agent no longer knows is given up as failed - told apart by a new
    `ExternalAgentError::UnknownTask`, which A2A's `TaskNotFoundError` becomes, since `Rejected`
    stands for any refusal, a passing one too.
4.  **A cancel reaches an open task**, in three steps, `RunControl` restructured first:
    *   **4a.** The tool calls a user interrupts on their own leave `RunControl` for a type of the
        agent's (`CallInterrupts`), each kept with its turn.
    *   **4b.** The turns set aside are kept by their turn, not their thread: an external agent
        may have several tasks working in its one thread. No change of behaviour.
    *   **4c.** A second kind of turn set aside (`Aside::GoingOn`, beside `WaitingForAPerson`): one
        whose work goes on elsewhere. An external agent sets the turn of a request it defers aside
        so, `resuming` the turns of its working tasks after a restart, and keeps them in step with
        its tasks at one point (`keep_going_on`); a cancel reaches them as it reaches a turn set
        aside, and the external agent cancels each task asked so and says what came of it so far,
        as a cancel of a request does.
5.  **External sub-agents.** The sub-agent tool no longer refuses an external personality
    (`SubAgentError::External` goes); tested end to end with a scripted remote agent.

## Done when

- An external agent can be started as a sub-agent: it is given the brief, and its answer reaches
  the main agent, also when its remote task ends after the turn that started it; covered by tests.

## History

- **2026-11-17** — Opened from issue 558, as the maintainer decided: external agents as
  sub-agents are a step of their own, after the first version.
- **2026-11-27** — What happens today with a late answer read in the code, and two approaches to
  it added, B recommended (*What happens today with a late answer*, *Approaches to a late answer*).
- **2026-11-27** — Corrected: what the section described is not what happens today - an external
  sub-agent is refused - but what would happen without the refusal (*What would happen with a late
  answer*); the first point of *The problem* noted as seemingly done since 557.
- **2026-11-28** — The maintainer decided for approach B (*Decision*), with the field on
  `TurnFinished`, and that a followed task is taken up again and a cancel reaches an open task in
  this work too (*Plan*). The first point of *The problem* found tested since 557.
- **2026-11-28** — Steps 1 and 2 done. For step 3 the maintainer decided that a task is given up
  only once its remote agent says it does not know it (`ExternalAgentError::UnknownTask`), and
  followed again otherwise, without limit.
- **2026-11-28** — Step 3 done. A first version of step 4 kept which tasks a cancel finds beside
  `RunControl` and had two races; the maintainer decided to restructure `RunControl` first, as part
  of this issue, before step 4 is built on it (put aside on the branch): steps 4a-4c.
- **2026-11-28** — Step 4a done. One change of behaviour came with it: `RunControl::stop` and
  `leave` cancel `ending` at once, so that whoever sees a turn's token cancelled finds the run
  ending - before, an external agent stopped in that moment could take the stop for a cancel and
  cancel a remote task the stop should have left working.
- **2026-11-28** — Step 4b done: the turns set aside are kept by their turn.
