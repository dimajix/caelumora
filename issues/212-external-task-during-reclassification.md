# 212 — An external agent's task that began before its channel was lowered is answered into the lowered channel

**Status:** in progress
**Area:** Confidentiality, integrations

## The problem

A turn of an agent run here keeps the channel's classification as it was when the turn began
(`TurnState::classification`). Before every step the `AgentActor` asks whether what the turn saw
may still flow into the channel as it is now (`turn_flow`); once the channel was lowered, the turn
is given up, nothing of it is published, and the users are told
([141](141-turn-during-reclassification.md)).

An external agent has no such check. Its `OpenTask` keeps no classification, and what comes back
is classified when it is published - as the remote agent's component declares, else as the channel
is *then* (`ExternalAgentActor::classification_in`). So:

1. The remote agent is given what the channel said while it was classified, say, `confidential`.
2. The channel is lowered to `internal` while the task works - for minutes, or across a restart
   as a followed task (`follow_open_tasks`).
3. The answer comes, is classified `internal` - the channel as it is now - and is published.

The answer was produced from what was `confidential`, and is shown to everyone who reads the
lowered channel. The same holds for the pieces published while an answer is written
(`PartialMessageGenerated`), and for a task that asks back.

A declared classification does not close it: it says what the remote agent produces on its own,
and is checked against the channel once, when the actor is built - not what it was given here.

## Why it matters

It is the case of issue 141 on the one path that issue left out, since external agents came later
([179](179-external-agents-via-a2a.md)). Lowering a channel is the operation that promises its
new readers that nothing above the new level reaches them; a task may be open for long, so the
window is wider than a turn's.

## Considerations

**Where the classification is kept.** An `OpenTask` gets the channel's classification as it was
when the task began, as a `TurnState` has it. [211](211-turn-and-agent-work-structure.md), step 5,
aligns the `OpenTask` with `TurnState` and is where the field fits; this issue adds it if it comes
first. A request answered without a task (the remote agent answers with a message at once) has no
`OpenTask`: the classification is then that of the exchange, held by the actor while it answers.

**What is done when the channel was lowered meanwhile.** Options:

1. *As a turn (recommended):* before anything of the task is published - a piece, the answer, a
   question back - ask `may_publish` for the classification it began with against the channel as
   it is now. If it may not: publish nothing, cancel the task at the remote agent, forget it, and
   tell the users with `TurnFailed` that the answer was discarded because the channel was lowered,
   in the words issue 141 uses. One rule for both kinds of agent.
2. *Publish it classified as it began.* The channel drops what is classified above it - in
   silence, which is the state issue 141 started from.
3. *Leave the task and publish when it ends, if the channel was raised again by then.* Keeps an
   answer nobody may see for an unknown time; no counterpart for agents run here.

**What the remote agent already heard.** The remote conversation keeps what it was given before
the lowering, and later tasks in it may tell of it. An agent run here keeps the same in its
transcript; how a lowered channel treats that is [67](67-change-channel-level.md)'s and
[141](141-turn-during-reclassification.md)'s answer, to be read first. If it is a gap there too,
it is one for both kinds and an issue of its own; if the transcript is cut or relabelled, the
external agent needs the counterpart - at the least a new remote conversation
(`RemoteState::conversation` forgotten) after a lowering.

**What is recorded.** A turn given up stays as a branch of the transcript. An external agent has
nothing to keep of an answer discarded; whether its history notes that an answer was discarded is
to be decided with [205](205-external-agent-message-context.md).

## Findings (2026-10-03)

Read against the code after [211](211-turn-and-agent-work-structure.md), which gave the `OpenTask`
the contexts of its request. Three things are wrong; the first is smaller than the problem above
says, the third was not seen when the issue was opened.

**1. What the remote agent was given is released by the lowering.** An external agent reads
nothing of ours but what it is given: the channel's transcript, per channel - the remote
conversation is per `AgentChannelState`. Lowering a channel releases what it holds (issue 67,
`MessageHistory::fit_into`), and agents carry what they know on; a turn of an agent run here
caps what its transcript adds to its label at the channel (`TurnState::see`). So the answer in the
example above is *not* made of anything the owner did not release. What 141 gives up a turn for -
what it read *elsewhere*, up to the classification it began with - has no counterpart here: the
remote agent's own knowledge is what its component declares (`ExternalAgent::classification`).
This also answers *What the remote agent already heard*: nothing is cut or relabelled for an agent
run here, so nothing is to be forgotten of the remote conversation, and no issue is needed for it.

**2. After a lowering, answers vanish in silence.** The classification of an answer is taken once,
when the exchange begins (`ExternalAgentActor::exchange`: `classification_in(channel now)`). A
lowering while it runs cancels the channel's actors (`ChannelLoop::reclassify` → `cancel_actors`);
the external agent then cancels the remote task and publishes what came so far - classified as
the channel *was*. The channel's gate drops it (`ChannelLoop::admits`), and with it the
`FullResponseGenerated` that ends the request: the web UI waits for an end that never comes, and the
agent's history keeps a message the channel never had. Pieces published between the lowering and
the cancel are dropped the same way. A task followed later (`speak`) is classified as the channel is
then, and published.

**3. A declared classification launders what the remote agent was given.** Its answers are
classified as its component declares, whatever it was given: a remote agent declared `public` in an
`internal` channel answers `public` with what it was told there, and an agent of a `public` channel
that reads that channel's history (issue 163) reads it. The check when the actor is built
(`may_publish(declared → channel)`) is not repeated after a lowering either: a declared
classification that no longer flows makes every later answer vanish as in 2.

## Plan

### The rule

An external agent's piece of work - one exchange, and the task it may begin - is treated as a turn:

- **Bound:** the channel's classification when it began, kept with it (`OpenTask::classification`;
  for an exchange without a task, held while it runs). As `TurnState::classification`.
- **Label:** what it writes as - the join of what it was given (the transcript sent) and what the
  remote agent brings in itself (its declared classification, or the bound if it declares none),
  never above the bound. As `TurnState::label`, with the same rule for a join that is not known to
  flow (`TurnState::see`). Pieces and answer are classified so.
- **Admission:** an exchange whose remote agent declares a classification that may not flow into
  the channel as it is now is refused at its start with `TurnFailed` - the check the actor makes
  when it is built, made once more per request, by one method.
- **Lowered:** when the bound may no longer flow into the channel as it is now
  (`may_publish(bound → channel)`, in the work's security context, as `AgentActor::turn_flow`), the
  work is given up: nothing more of it is published, the remote task is cancelled and forgotten,
  its history keeps nothing of it, and the users are told with `TurnFailed` in the words
  `AgentActor::give_up` uses for `GivenUpReason::Lowered`.

The rule gives up answers made only of what was released (finding 1). It is chosen anyway, for one
reason the users see: the web UI tells whoever lowers a channel that answers being written are
discarded (issue 141), and one rule for both kinds of agent is what that warning can say.

### Where it is asked

As for a turn, at the points where work goes on, not at every piece:

- **An exchange,** when it is cancelled (`token.cancelled()` in `exchange`): a lowering cancels it.
  If the bound no longer flows, it is given up instead of answered with what came so far.
- **A followed task,** before what came of it is said (`speak`), and before the actor follows it
  again (`follow_open_tasks`): a lowering while the actor did not run - or between two runs - is
  found there.
- **A task that asked back,** when the next request takes it up (`exchange`, `asking`): if its
  bound no longer flows, it is cancelled and forgotten, and the request begins a new task.

What is published between the lowering and the cancel is classified by the label, not the old
channel: it passes the gate when it may, and is dropped only when it is above the lowered channel -
the same window a turn has (issue 141).

### Types and APIs

No new type. `Classification` gets the rule `TurnState::see` holds today, so that both use it:

```rust
impl Classification {
    /// `self` joined with `other`, where the join is known to flow into `bound`; else `bound`,
    /// which everything seen within it flows into.
    pub fn join_within(&self, other: &Classification, bound: &Classification) -> Classification;
}
```

`OpenTask` (`caelumora-model/src/external_agent.rs`) gets two fields, as `TurnState` has them:

```rust
pub struct OpenTask {
    // ... as after issue 211
    /// The channel's classification when it began - as `TurnState::classification`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    /// What it writes as - as `TurnState::seen`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<Classification>,
}

impl RemoteState {
    /// As after 211, with the bound and the label of the work that began it.
    pub fn begin(&mut self, task: &TaskId, work: &Work);   // see below
    pub fn work_of(&self, task: &TaskId) -> Option<Work>;  // replaces contexts_of
}
```

Here lies the one decision of this plan (see *Decide*): the four values a piece of work is done in -
event context, security context, bound, label - travel together between the exchange, `begin`,
`speak` and the checks. Passed as four parameters and returned as a four-tuple they are no longer
"two values to a few callers"; a struct `Work` in the model would hold them, with `label()`,
`see()` and nothing else.

In the runtime (`actor_impl/external_agent.rs`):

- `ExternalAgentActor::may_answer_in(channel) -> bool` - the admission check, used by `new` and by
  every exchange.
- `ExternalAgentActor::flows(work, channel) -> bool` - the lowering check, as `turn_flow`.
- `ExternalAgentActor::give_up(state, task, work, output)` - cancel, forget, `TurnFailed`.
- `classification_in` goes; `record` and `message` take the label.
- The text told for a lowering moves to `actor_impl/mod.rs` (`lowered_message(name, channel)`),
  used by both actors.

### Steps

1. **`Classification::join_within`**, used by `TurnState::see`. No change of behaviour.
2. **Bound and label** (finding 3): an exchange takes its bound and label when it begins, checks
   admission, classifies pieces and answer by the label; `RemoteState::begin` keeps both with the
   task, `speak` uses them. Changes behaviour: a declared classification no longer lowers what the
   remote agent was given; a declared classification that no longer flows is refused with
   `TurnFailed` instead of vanishing. A task stored before has no bound: it is classified as the
   channel is when it speaks, as today.
3. **Lowered work is given up** (finding 2) at the three places above, with `lowered_message`
   shared. Changes behaviour: after a lowering, an external agent's answer under way and its open
   tasks begun before end with `TurnFailed`, and their remote tasks are cancelled.
4. **Documents:** CONFIDENTIALITY.md (the rule holds for an external agent's work), CHANNELS.md
   (bound, label, what is given up), the web UI's warning unchanged.

Each step a commit of its own.

### Tests

- `confidentiality.rs`: `join_within`, the cases `TurnState::see`'s test has.
- `channel.rs`, with `ScriptedExternalAgent`:
  - a declared `public` agent given an `internal` message answers `internal`;
  - a request to an agent whose declared classification no longer flows is answered with
    `TurnFailed`, and the remote agent is not called;
  - lowered while it answers: nothing of the answer is published, `TurnFailed` is, the remote task
    is cancelled, the agent's history keeps nothing;
  - a followed task begun before a lowering: cancelled, nothing said, `TurnFailed`;
  - a task that asked back before a lowering: the next request cancels it and begins a new one.
- Security suite: lowered while bob's task works - the check is asked in bob's name (`bob as
  hermes: Publish no`), and the remote agent is asked to cancel in his name.

### What it costs

About 20 lines in the model (method, two fields, `Work` if decided), 120 in the external actor,
10 shared with the `AgentActor`, 250 of tests. Stored states change only by two optional fields.

### Decide

1. **The rule:** give up as a turn (recommended, above), or publish what was released - classified
   by the label and admitted by the gate, with only findings 2 and 3 fixed. The second loses no
   answers, and needs the web UI's warning to say that external agents' answers are kept.
2. **Finding 3 here or in an issue of its own.** Recommended here: it is the same question - what
   an external agent's work writes as - and step 3 needs the label of step 2.
3. **`Work` or parameters.** Recommended: the struct, as the counterpart of what a `TurnState` holds
   of its contexts; `OpenTask` would then hold an `Option<Work>` instead of four optional fields.
   It is the one new type; without it, four values go through five functions.

## Done when

- An open task keeps the channel's classification as it was when it began.
- An answer, a piece of one, or a question back of a task whose classification may no longer flow
  into its channel is not published; the task is cancelled at the remote agent, and the users are
  told as for a turn.
- The same holds for a task followed after a restart.
- A test in the security suite covers both: answered while asked, and followed.
- What the remote conversation heard before the lowering is decided - here, or in an issue this
  one names.
- CONFIDENTIALITY.md says that the rule for a turn holds for an external agent's task.

## History

- **2026-10-03** — Issue created. Found while reading `OpenTask` against `TurnState` for
  [211](211-turn-and-agent-work-structure.md): the `OpenTask` keeps neither whose authority it
  acts with nor the classification its channel had when it began. Read from the code, not yet
  reproduced in a test.
- **2026-10-03** — Read against the code after 211, and planned (*Findings*, *Plan*). The leak the
  issue was opened for is smaller than described - what the remote agent was given is released by
  the lowering - but answers after a lowering vanish in silence, and a declared classification
  launders what the remote agent was given. Three decisions are left to the maintainer
  (*Decide*).
- **2026-10-03** — Decided with the maintainer, as recommended under *Decide*: an external agent's
  work is given up as a turn when its channel is lowered; finding 3 is fixed here; the four values
  a piece of work is done in become one struct `Work` in the model, and `OpenTask` holds an
  `Option<Work>`.
- **2026-10-03** — Step 1 done: `Classification::join_within(other, bound)` holds the rule
  `TurnState::see` had - the join where it surely flows into the bound, else the bound - and
  `TurnState::see` uses it. No change of behaviour.
- **2026-10-03** — Step 2 done: `Work` (`caelumora-model/src/external_agent.rs`) holds the contexts
  of a piece of work, its bound (`classification`) and its label (`seen`, with `label` and `see` as
  `TurnState` has them); `OpenTask::work` replaces the two context fields of 211, and
  `RemoteState::begin(task, work)` and `work_of` replace `begin(task, contexts)` and `contexts_of`.
  An exchange begins its `Work` with the channel as it is, checks admission
  (`may_answer_in`, shared with the actor's constructor) and sees what the remote agent brings in
  - as declared, else the bound - and every message of the transcript; its pieces and answer are
  classified by the label, and `speak` uses the task's. `classification_in` is gone. Changed with
  it: a declared classification no longer lowers what the remote agent was given (a `public`
  agent told what is `internal` answers `internal`); a request to an agent whose declared
  classification no longer enters its lowered channel is answered with `TurnFailed`, in the name
  of who asked, and the remote agent is not called. Open tasks stored with the two context fields
  of 211 lose them and act as the agent itself. Tests: `Work` in the model, the label in
  `channel.rs`, the refusal in the security suite.
- **2026-10-03** — Step 3 done: work whose bound may no longer flow into its channel
  (`ExternalAgentActor::lowered`, `may_publish` in the work's name) is given up at the three places
  of the plan - an exchange cancelled by the lowering ends as `Ending::Lowered`, nothing of it
  published or kept; a task to be followed, or one whose end is to be said, is cancelled at the
  remote agent if it has not ended, forgotten, and the users told (`give_up_task`); a task that
  asked back is cancelled when the next request would take it up, which begins a new one.
  `lowered_message` in `actor_impl` is the text both actors tell. The channel tests of external
  agents run under `FlowRule` instead of a policy that permits everything, so that a lowering is
  one. Tests: three in `channel.rs`, one in the security suite - where the remote task is
  cancelled, as every cancel does, before it is asked whether it may go on. Not tested: a task
  followed while the channel is lowered, whose end is then given up in `on_followed` - the scripted
  remote agent cannot hold back its events until the lowering.

