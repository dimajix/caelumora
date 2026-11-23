# 602 — An actor takes part in one thread of its channel alone

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Every actor of a channel - an agent's, a user's - takes part in exactly one of its threads (issue 557), so an agent of the line cannot take part in a thread a person opens at a message of the line, a user writes only in the thread they take part in, and a nudge of someone in another thread asks nobody - though 557 decided one actor per agent and channel, with a head of its transcript per thread.

## The problem

Issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md) built threads for what
[558](558-no-agent-hands-a-task-to-a-sub-agent.md) needed first, and in that cut "no agent holds
two threads" (557, *The cut: what 558 needs first*): a sub-agent is a new agent, taking part in its
own thread alone, and the main agent takes no part in it. The code holds one thread per actor:

- `ChannelActor::thread` (`channel/mod.rs`) is one `ThreadId`, and `Channel::adopt` adds the actor
  to the moderator of that thread alone.
- `Recipients::goes_to(actor, thread, happened_in)` delivers by that one thread.
- `ActorRuntime::thread_id` and `ParticipantRuntime::thread_id` name it; what an actor says of
  itself - `Answering::own` in `AgentBody::own` and in `UserActor::envelope` - happens there.
- Opening a thread refuses a founder that takes part in the channel already
  (`ChannelError::AlreadyParticipant`, `session/threads.rs`), "since an actor takes part in one
  thread" (CHANNELS.md).
- After a restart every actor starts again in the thread its range of attendance names.
- An agent's `AgentChannelState` holds one transcript, one turn and one `processing_state`.

What follows from it:

- **Threads that people open have no agent in them.** The use 557 was opened for - a side question
  or a second approach, followed beside the line with the agents of the line - cannot be built: an
  agent of the line cannot take part in a thread as well.
- **A user writes in one thread.** `UserActor::send_message`, `request_response` and the requests
  to answer again carry the thread of the user's runtime, and the REST API passes none; a user
  reads every thread but speaks in their own, the line.
- **A nudge across threads asks nobody.** A user's nudge happens in the user's thread, and is
  routed by that thread's moderator (`ChannelLoop::enter`); `PolicyModerator::asking` asks the
  participant only if it takes part there (`takes_part`). An agent in another thread is admitted as
  someone who runs in the channel, its thread's quiet is ended (`route_event`), but nobody is asked
  and nobody woken. CHANNELS.md (*Nudging someone*) speaks of "the thread the one it asks takes
  part in", as if this worked. Found by reading the code, not held by a test; it arises whenever a
  user nudges a sub-agent. A request to answer again goes the same way.

## What is decided already

In 557, by the maintainer:

- **One actor per agent and channel, a head per thread** (*Threads in the running channel*,
  2026-11-16): the agent's transcript keeps a branch with a head of its own for every thread it
  takes part in. No loop of its own per thread.
- **No turns at once across threads**: an agent works on one turn at a time, in whichever thread.
- **`AgentChannelState` splits** (*What changes*) into what is the channel's - notes, private
  notes, `heard`, `last_message`, the prompt texts and tool sets kept, the numbers of attachments -
  and a state per thread: the head of its branch, its window, its turn and `processing_state`, and
  how full its context is.
- **Forked and fresh threads** (*Decided on 2026-11-17*): in a forked thread, as a person opens
  one, the agents' branches fork from where they stand in the thread's origin; in a fresh one they
  begin at the root.
- **A person branches once**, at a message of the line; a thread has a responder policy of its
  own; hops between agents are counted per thread.
- **Agents hear by thread, users the whole channel** (D5): the one deliberate difference in what
  is delivered, since what an agent hears enters its context and is paid for by every call after
  it.

Proposed in 557 and not decided (*Proposal for threads*): who takes part in a thread - whoever
opens it, the author of the message it hangs at, whom its first message mentions; later a person by
writing in it, an agent by being mentioned in it; in a closed circle nobody beyond those fixed at
its opening. Open in 557: D7, the shape of a transcript with several threads, and
`AgentBody::drop_answered` ("holds as long as an agent hears one thread; with several, step 3").
The step 3 those name has no issue of its own; this is it.

## Considerations

The context of a turn is not built anew per thread: `Turn::full_context` resolves it through
`ContextResolver` from the agent's state, and keeps doing so. What changes is which branch of the
transcript every reader names; the notes, the prompt parts and the tool sets stay the channel's.

### The first user

A person opens a forked thread at a message of the line, and an agent of the line takes part in it,
in the line as well. Later, each on its own: an agent of a thread in a thread nested below it,
external agents in several threads (a `RemoteState` per thread), unloading threads (with
[381](381-inactive-channels-held-in-memory.md)), and the view of a thread in the frontends
([595](595-no-frontend-shows-another-thread-of-a-channel.md)).

### One way of taking part, for users and agents

A participant takes part in a set of threads, whatever its kind: what tells a user from an agent is
what is delivered to it (D5), not how it takes part. `Attended { thread, range }` is a range of one
thread already, and a participant's attendance a list of them, which `Recipients::reaches` reads:
taking part in two threads at once is two ranges that overlap. A join and a leave of a thread are
of that thread (557, *What is of a thread*), so taking someone into a further thread is a
`ParticipantJoined` in it, and letting them go from it a `ParticipantLeft` there.

### Steps, by the author - not decided

Their order was decided on 2026-11-21, with a step before them (*Decided on 2026-11-21*, below D7).

1. **Restructuring, without a change of behaviour: `AgentChannelState` splits.** The part of a
   thread becomes a type of its own, one entry for the agent's one thread:

   ```rust
   /// What an agent keeps of one thread of its channel: its branch of the conversation and its
   /// turn there.
   pub struct AgentThreadState {
       // transcript, transcript_from, turn, processing_state, used, remote
   }

   impl AgentChannelState {
       /// The agent's state in `thread`, `None` if it takes no part there.
       pub fn thread(&self, thread: ThreadId) -> Option<&AgentThreadState>;
       /// As `thread`, to change it.
       pub fn thread_mut(&mut self, thread: ThreadId) -> Option<&mut AgentThreadState>;
   }
   ```

   `ThreadState` names a thread's part of `ChannelState` already. What touches a thread's part
   alone moves to `AgentThreadState` - compacting, beginning and finishing a turn, answering again,
   keeping the window -; what touches both - `said`, `perceive`, with `last_message` and the
   numbers of attachments - is given the thread. `ContextResolver` reads the channel's part and the
   thread's. Whether a thread's part is a document of its own, per agent, channel and thread, as
   557 proposed, follows D7.

2. **Restructuring, without a change of behaviour: taking part is a set, of one thread.**
   `ChannelActor::thread` becomes `threads`; `Channel::adopt` adds the actor to the moderator of
   each; `goes_to` takes the actor's threads; the runtime's `thread_id` becomes
   `fn threads(&self) -> Vec<ThreadId>`, and whoever calls `Answering::own` names the thread - in a
   turn the thread of its work, as `Answering::continuing` does already. An actor starts again in
   every open thread of its attendance. The agent's intake goes by thread:
   - `take_in` takes up to a request to respond of a second thread and leaves it where it is, its
     position not heard, so that the oldest request is answered first, and in each thread the last
     one, everything before it heard - today's rule, per thread -, and no open request has to be
     stored;
   - during a turn in a thread, it takes ahead what happens in that thread alone and passes over
     the others (`Pick::PassOver`, which the `EventReceiver` has);
   - `drop_answered` drops the next event of the turn's thread, passing over the others;
   - `perceive` appends to the branch of the thread a message was said in; what happens in the
     whole channel, and a change of a message, reaches every branch that holds it.

3. **Taking into a further thread.** A participant of the channel is taken into an open thread as
   well, and let go from one thread alone, by procedures of
   [593](593-every-lifecycle-change-is-guarded-in-a-way-of-its-own.md)'s pattern; letting go from
   the channel lets go from every thread, and a thread is closed once nobody takes part in it, as
   now. Opening a thread no longer refuses whoever takes part in the channel already. Who is taken
   in when is 557's proposal above, to be decided here; a thread delegated into stays a closed
   circle. The agent's new branch is forked or fresh, as the thread is. With it:
   - **cancelling a thread** ends the turns that run in that thread, not the turn of every actor
     taking part in it, which may run in another;
   - **a nudge and a request to answer again name a thread**, and are routed by its moderator,
     which resolves the nudge across threads above;
   - **a user writes in a thread they name**: `send_message`, `request_response` and the REST API
     take one - which overlaps 595's open point of writing into a thread;
   - **an external agent** keeps to one thread, and is refused a second, until it has a remote
     state per thread.

### D7 - the shape of a transcript with several threads

Read in the code: a `Position` counts the entries of a chain across every branch, and a window is a
suffix of the chain from a position on (`Chain::keep_from`, `Chain::suffix`). 557 named two shapes:

- **One chain, a head per thread.** A fork is a branch, and the store stays as it is. But the
  branches interleave by position: a window holds everything from the earliest beginning of any
  thread's window on, and one thread cannot be let go alone. `Chain` holds one `head` and one
  `path` today, and would have to hold one per thread.
- **A chain per thread.** Each has positions, a window and a head of its own, and every reader of a
  `Transcript` stays as it is, given its thread's chain. The cost lies at the fork: the first entry
  of a forked thread follows a digest in its origin's chain, so `TranscriptStore::path` follows the
  path into the origin's chain, and `ContextResolver` reads the origin's prefix as long as the
  thread's window reaches back to the fork.

The author recommends a chain per thread: the window ([380](380-transcript-window.md)), compaction
and later unloading stay per thread without a change of the chain's model of positions, and the
cost lies in one place. Before deciding, `AgentChannelState::compact` and `ContextResolver` are to
be read for what a fork across chains asks of them.

#### Proposal: one tree, a head per thread (2026-11-21, not decided)

Worked out with the maintainer; it takes the place of the recommendation above. Decided on
2026-11-21 (*Decided on 2026-11-21*, below).

**What reading the code showed.** A chain per thread does not leave the readers of a transcript as
they are. `Transcript::compact`, `Transcript::carried_parts`, `AgentChannelState::retried` and the
`ContextResolver` of a state read the active path from its root (`Transcript::entries`); the chain
of a forked thread lacks its origin's prefix until its first compaction, so every one of them would
read across two chains, and across more with nested threads. What the author held against one
chain lies not in its model but in how a `Chain` is held in memory: a range of positions from
`base` on, every branch in it.

A `Chain` is a tree already: an entry follows the one it names by its digest, a compaction begins a
branch at a root of its own, and the old branch stays. A person opens a thread after A3, and the
line is compacted later:

```
A1-A2-A3-A4-A5          the line's old branch
         └-B1-B2-...    thread B, forked after A3
A5'-A6-A7-...           the line after the compaction, from a root of its own
```

B's path, A1-A2-A3-B1-B2, lies in the one tree, whole; a compaction of B reads it as it reads the
line's today.

**The transcript is a tree of the conversation with several threads.** A type of its own, not a
`Chain` - the history, notes and memory keep `Chain`, which needs neither several heads nor paths
in place of a range -, with the same nodes and the same stored form. It holds:

- the position the next entry is given - the one counter, shared by every thread;
- a head per thread, by `ThreadId`;
- in memory the paths those heads reach, each from its root: a thread's window. What no head
  reaches is let go, and a thread is unloaded by letting go of its head;
- what was added since it was stored.

Every entry is appended through it, naming its thread (`push(thread, message)`), so that positions
are given in one place; a reader is given the path of its thread and reads it as it reads
`entries()` today. `AgentThreadState` (step 1) holds the turn, `processing_state`, how full the
context is, later the `RemoteState` - no part of the transcript. `transcript_from` goes.

**Stored.** The nodes stay as they are, by position, in a tree; a thread is no key of the store.

- `Node` keeps the position of the entry it follows beside its digest. It is covered by no digest.
  `Node` is generic, so the history, notes and memory gain it as well, and their `path` with it.
- The stored head becomes a head per thread, each with its position; the chain's end stays.
- Writing stays as it is: the nodes, then the heads, so that no head names a position past the end.

**Loading a thread** follows its path from its head back to its root, by the position of each
entry's predecessor, and reads only what the path touches; the threads of an agent are loaded
together, what they share read once. In the JSON store a position `p` lies in the segment from
`p - p % SEGMENT`, at its index from there; a segment is read whole. So the line reads no more than
`load` reads today - every segment from `transcript_from` on -, and an old thread as many segments
as its path touches, however far the line went on. A segment full of other threads' large entries
is read with them: the size of the segments is the lever, and an index per thread waits until it
is measured to matter. In Postgres the path is read by the positions of the predecessors instead of
in batches backwards; whether an index on the chain's key and `seq` exists was not checked. Today
both stores search a path backwards by digest (`follow_back`) - through everything added since.

**Positions** stay what they are: given once, without a gap, checked by the store (`fits`), with
one writer, the agent's actor in the channel. The new counter replaces `base` plus the number of
nodes held, which only a range has. Two threads giving the same position are refused by the store,
not stored. `Chain::push_with` does not add an entry whose digest it holds, but moves the head onto
it; the transcript does so over every path it holds, else the same digest is stored twice - which
the store bears ("the store keeps both", `kept_chain.rs`).

**Why not a chain per thread** beside the readers above: a digest names its path on whichever
branch, so `TranscriptStore::path`, `message` and `in_transcript` - the context panel, what was not
passed on - stay as they are; with a chain per thread every `ContextRef` would need its thread, or
the store would search every chain. Only loading a window names a thread. The history already lives
so: one sequence of positions across every thread of a channel, a thread found through its entries
(`Filter::in_thread`), with no key per thread in the store.

**What it costs.** A type of its own for the transcript; a field of `Node`, so the stored chains of
every kind are converted or deleted; a head per thread in the stored form. D7 is decided before
step 1, which changes what is stored already.

**Open.** Which readers read more than the active path from memory - `Chain::all_entries`,
`Chain::from`, `Node::active_child` - and go to the store instead or go; and whether
`active_child`, where a switch to a branch goes on, is needed at all once every thread has a head
of its own and answering again moves one back.

#### Decided on 2026-11-21

D7 is decided as proposed: one tree, a head per thread. Read in the code before deciding, and
correcting the proposal where it says otherwise:

- **No reader goes beyond the active path.** Outside tests, nothing reads `Chain::all_entries`,
  `from`, `at`, `leaf_from` or `children` of a transcript; the head moves only in
  `Transcript::start_anew`; answering again takes messages back (`Transcript::revoke`) and moves no
  head.
- **Every place that appends knows its thread**: in a turn the thread of its `Work`
  (`event_context.thread_id`), in taking in the thread of the envelope. What happens in the whole
  channel goes on every branch, and a change of a message on every branch that holds it.
- **The position of a predecessor** helps loading a thread, whose head has a position; `path`
  still looks for its first node backwards by its digest, so for the history and the notes it
  gains little.
- **Postgres** has the primary key `(agent_id, channel_id, seq)` on `transcript_entries`: a position
  is read by an index.

Decided with it, by the maintainer:

- **A - a reader names its thread as a parameter** of every method that reads or extends a
  thread's branch. A handle per thread (`branch(thread)`) was weighed and not taken: it only
  bundles a reference and a thread.
- **B - the heads are stored in the chain's head record** (`head.json`, the table `transcripts`),
  so that a crash leaves what it leaves today; the stores of chains are given a head of their own
  per kind of chain.
- **C - the position of a predecessor comes later**, once threads run side by side or loading is
  measured to be slow: it changes the stored form of every chain. Until then a thread is loaded
  backwards from the position of its head to its root.
- **D - the branch memory of `Chain` goes first**, as a step of its own: issue
  [394](394-chain-branch-memory.md), decided anew - threads take the place of branches to switch
  between. With it a node never changes once stored.

The interface of the transcript, agreed:

```rust
/// What an agent's model is given of its channel: a tree of entries with a head for every thread
/// the agent takes part in (issues 380, 557, 602), each thread's branch the path from its root up
/// to its head.
pub struct Transcript { .. }

impl Transcript {
    /// The digest of the branch of `thread`, `None` while it is empty.
    pub fn digest(&self, thread: ThreadId) -> Option<Digest>;
    /// The entries of the branch of `thread`, from its root up to its head.
    pub fn entries(&self, thread: ThreadId)
        -> impl DoubleEndedIterator<Item = &Node<TranscriptEntry>> + ExactSizeIterator + Clone;
    /// The last entry of the branch of `thread`.
    pub fn last(&self, thread: ThreadId) -> Option<&Node<TranscriptEntry>>;
    /// The messages of the branch of `thread` as they stand.
    pub fn messages(&self, thread: ThreadId) -> Vec<Message>;
    /// The parts of prompt components on the branch of `thread` a compaction carries over.
    pub fn carried_parts(&self, thread: ThreadId) -> HashSet<Digest>;
    /// The path from its root up to the entry `digest`, on whichever branch it holds it.
    pub fn prefix(&self, digest: &Digest) -> Option<Vec<&Node<TranscriptEntry>>>;

    /// Appends `message` to the branch of `thread`; returns the digest before it.
    pub fn push(&mut self, thread: ThreadId, message: Message) -> Option<Digest>;
    /// Appends `message` to the branch of `thread`, given to read as `source`.
    pub fn give(&mut self, thread: ThreadId, message: Message, source: Option<GivenSource>)
        -> Option<Digest>;
    /// Appends `entry` to the branch of `thread`.
    pub fn record(&mut self, thread: ThreadId, entry: TranscriptEntry) -> Option<Digest>;
    /// Begins the branch of `thread` anew at a root of its own; returns its digest before.
    pub fn start_anew(&mut self, thread: ThreadId) -> Option<Digest>;
    /// Begins the branch of `thread` anew as a compaction does.
    pub fn compact(&mut self, thread: ThreadId, replacement: Vec<(Message, Option<GivenSource>)>,
        keep_from: Option<Digest>) -> Option<HashMap<Digest, Option<Digest>>>;

    /// Records `modified` on every branch that holds its message; whether one did.
    pub fn modify(&mut self, modified: Message) -> bool;
    /// Whether a branch holds the message `modified` changes.
    pub fn takes(&self, modified: &Message) -> bool;
    /// Records the message `id` as taken back at `at` on every branch that holds it.
    pub fn revoke(&mut self, id: MessageId, at: DateTime<Utc>) -> bool;
    /// Records the message `id` as reclassified on every branch that holds it.
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool;

    /// The position the next entry is given, on whichever branch.
    pub fn end(&self) -> Position;
    /// The entries added since it was last stored - those no head reaches any more among them -
    /// and the head of every thread; `None` if nothing changed.
    pub fn changes(&self) -> Option<StoredTranscript>;
    /// Marks what `changes` told as stored.
    pub fn mark_stored(&mut self);
    /// Lets go of the entries no head reaches, of those stored alone.
    pub fn keep_window(&mut self);
    /// The transcript as a store loaded it: the branch of every thread from its root, and the
    /// position after the last entry stored.
    pub fn loaded(end: Position, branches: BTreeMap<ThreadId, Vec<Node<TranscriptEntry>>>)
        -> Result<Self, String>;
}

/// A transcript as it is stored, and what it tells a store since it was last stored: the entries
/// added, on whichever branch, and the head of every thread with an entry, by its position.
pub struct StoredTranscript {
    pub entries: Vec<Node<TranscriptEntry>>,
    pub heads: BTreeMap<ThreadId, Position>,
}
```

Letting go stays a step of its own after a put (`keep_window`): every position given has to reach
the store, which takes no gap (`new_nodes`), and the history's `ContextRef`s name entries that no
head reaches any more - an old branch after a compaction, an external agent's call before. Around
the type, `AgentChannelState::transcript_from` and its change go, `ContextResolver::of` is given the
thread, and a store loads a transcript whole, without a position (`load_transcript(agent,
channel)`); `TranscriptStore` stays as it is.

The steps, in their order: **D**, the branch memory removed (394); **1a**, the transcript a type of
its own, with one thread; **1b**, `AgentChannelState` split (step 1 above); **2** and **3** as
above. The position of a predecessor (C) is no step yet.

### Further points

- **After 593.** Step 3 changes the lifecycle procedures 593 builds - letting someone go holds the
  thread they take part in, a thread is closed once nobody takes part -; it waits for them.
- **A change of a message from before a fork** reaches every head whose path holds the message
  (557, *What is still hard*).
- **Stored data**: the agents' states and transcripts are kept per thread from step 1 on; what is
  stored has to be converted or deleted.
- **593 is done** (2026-11-21), so step 3 waits for nothing: taking someone into a further thread
  and letting them go from one are procedures beside `participant::take_into_channel`
  (`caelumora-kernel/src/procedure/participant.rs`). Opening a thread moved with it: the refusal
  of a founder who takes part already, `ChannelError::AlreadyParticipant`, is in
  `procedure/thread.rs` (`open`), no longer in `session/threads.rs`, which is gone.
- **A turn that waits blocks every thread.** With one turn at a time, an agent whose turn in one
  thread waits for a person (`ActorState::WaitingForUser`: an approval, a question) answers in no
  other thread until it is answered. 557 decided one turn at a time but does not say what a
  waiting turn means for the others. It concerns the first user at once: a side question whose
  turn waits for an approval silences the agent in the line as well. Two ways: it is accepted, and
  shown - 557 has the web UI show where an agent is answering -; or a turn that waits does not
  count as the one under way and is set aside until it is answered, which reopens 557's decision.

## To decide

- D7, the shape of a transcript with several threads.
- Who is taken into a thread, and when (557's proposal).
- The cut of the first user above, and the steps.
- Whether a nudge across threads is fixed before this issue - by routing it with the moderator of
  the thread of whom it asks -, or with step 3.
- What a turn that waits for a person in one thread means for the agent's other threads.

## Done when

- An agent takes part in several threads of its channel, with a branch of its transcript and a turn
  state for each, one turn at a time.
- A user writes in a thread they name, and a nudge reaches whom it asks in the thread it names.
- D7 and who is taken into a thread are decided and recorded here.

## History

- **2026-11-19** — Opened from 557's step 3, which 557 named and left without an issue, and from a
  nudge across threads found while reading how threads are routed.
- **2026-11-21** — Proposed for D7, with the maintainer: one tree, a head per thread, the transcript
  a type of its own that gives every position, a node keeping the position of its predecessor, and
  a thread loaded by following its path (*Proposal: one tree, a head per thread*); not decided.
  Recorded under *Further points*: 593 is done, so step 3 waits for nothing, and where opening a
  thread moved; and that a turn waiting for a person blocks the agent in every thread, which joins
  *To decide*.
- **2026-11-21** — Decided by the maintainer: D7 as proposed, and A to D with the interface of the
  transcript (*Decided on 2026-11-21*), which corrects what the proposal said of the position of a
  predecessor and of Postgres. Issue 394 decided anew and done as step D: `Chain` no longer
  remembers the branch last taken, and a node never changes once stored.
- **2026-11-21** — Step D committed. Step 1a done: `Transcript` is a type of its own, a tree with
  a head per thread, as agreed. Beside the interface, and kept by the maintainer: `extend`, which
  taking in uses, and `Situation::branch_thread_id`, the thread whose branch a work reads. Decided
  by the maintainer, deviating from *Decided on 2026-11-21* until step 2: an agent's branch is the
  one of the thread of its runtime (`ActorRuntime::thread_id`) - what it takes in, answers again
  and does in a turn, whatever thread the envelope or the work names -, since a request to answer
  again or a command reaches an agent from any thread, and read in the thread of the request it
  was refused or began on an empty branch. Step 2 brings the thread of the envelope and of the
  work. The stores of chains have a head of each kind (`KeptChain<E, H>`, `ChainHead<H>`,
  `Chains<E, H>`), a transcript is loaded by following each head back to its root (`path_to`), and
  Postgres keeps every head as JSON. `transcript_from` is gone. Also decided: `Chain::move_head`
  and `Chain::prefix`, left without a caller outside tests, go in a step of their own after 1a,
  with them the branches of `Chain`.
- **2026-11-21** — Step 1a committed. The step after it done: `Chain` no longer branches. Every
  entry follows the one added before it, and the head is the last entry; `move_head`, `prefix`
  and `all_entries` are gone, `entries` reads everything held, `apply` refuses an entry that does
  not follow the one before it and merges an older change without effect, as a journal replays
  it, and `push_with` no longer dedupes. The history, notes and memory read the same as before,
  since nothing outside tests moved their heads; their stored form is unchanged, the head kept
  beside the nodes. The transcript is the one tree.

- **2026-11-22** — The chain step committed. Decided by the maintainer for step 1: 557's split as
  decided - a turn and `processing_state` per thread -, since a turn that waits for a person is to
  be set aside while the agent answers in another thread, which settles that point of *To decide*:
  one turn at a time that works, the rule itself with step 3. The transcript stays on
  `AgentChannelState`, a head per thread (D7), and with it what touches both the transcript and a
  turn, given the thread; `remote` stays too, until an external agent has a state per thread. Also
  decided: `thread_mut` begins a thread's part where there is none, a part goes with step 3 as
  someone is let go from a thread; what reads without a thread reads every thread's, and what the
  agent used in the channel is the totals of its threads with the context of the fullest; whether
  `Sleeping` leaves `ActorState` for the actor as a whole is decided with step 3. Step 1b done:
  `AgentThreadState` holds a thread's `processing_state`, `turn` and `used`, in
  `AgentChannelState::threads`; `prepare_resume` and `has_work_under_way` are its own, read over
  every thread by the channel's; `offer_tools` is given the thread; the journal records every
  thread's part whole (`Threads`); a store loads what the turns of all threads name
  (`load_named_by_turns`, `TranscriptStore::named_by` of several contexts). The actor reads and
  writes the part of its runtime's thread until step 2, `RunControl` one per actor until step 3.
  The stored agents' states are to be deleted.
- **2026-11-22** — Step 1b committed. Decided by the maintainer, as recommended: step 2 goes in
  two, 2a the agent side - it reads the thread of the envelope and of the work - before 2b,
  taking part a set of one thread in the kernel; an envelope of a thread the agent takes no part
  in goes on the branch of the one it takes part in, one rule in one place, until step 3 routes a
  nudge, a request to answer again and a command by the thread they name; where something is said
  of oneself without a thread to name - the tools an agent would be offered, what a user says -,
  the thread one was taken in until step 3; and `AgentChannelState::turn`, `turn_mut` and `turns`
  in place of looking a turn up in every reader. Step 2a done: `AgentBody::branch_of` is that
  rule; a turn runs on the branch it was begun on (`Turn::of` and `Situation::new` are given it)
  and the actor keeps the thread of the turn that runs or ran last (`RunControl::thread`, both
  begun by `AgentBody::begin_turn`); taking in goes by branch - a request to respond on another
  branch ends the batch and waits unheard, and during a turn and as it ends what happens on
  another branch is passed over -, which with one thread changes nothing; `prepare_resume` tells
  each turn what its model is given of a call not executed in the variant that turn was begun
  with. An external agent keeps to its one thread. Testable only once an agent takes part in two
  threads, with step 2b: the batch ended by a request of another branch, `build` resuming and
  `store` writing in a thread other than the runtime's.
- **2026-11-22** — Step 2a committed. Step 2b done: taking part is a set, of one thread
  (`ChannelActor::threads`, `ActorRuntime::threads`, `ParticipantRuntime::threads`); `goes_to`
  takes the set, `HistoryStore::attending` gives each actor the threads of its open ranges, and
  `threads_taken_part_in` the threads to hold. A join is announced in the thread joined in, a
  leave in each thread taken part in, which `Attendance` closes as one range until step 3; a nudge
  and a request to answer again end the quiet of each thread. Named until step 3:
  the thread one joined the channel in (`joined_thread_id`), where one says of oneself what names
  no thread - what a user says, the tools an agent would be offered, an external agent's one
  thread -; the channel picks it from the attendance in one place (`ChannelLoop::joined_in`), as
  nobody is taken into a further thread yet, and an actor is still taken in in that one thread
  alone (`adopt`). Where an agent stands is stored and announced in the thread of its turn, else
  in the one it joined in (`AgentBody::standing_in`); what it recorded is told in the thread of
  its branch (`to_tell`). With one thread nothing changes; the tests of 2a's rules with two
  threads, which 2a could not have, are added.
- **2026-11-22** — Step 2b committed. Decided on the maintainer's word to decide for a clean and
  consistent architecture and the behaviour expected: step 3 goes in four - 3a, taking a participant
  of a channel into a further thread and letting them go from one; 3b, saying something, nudging,
  asking to answer again and a command name the thread they happen in, over REST as well; 3c, who is
  taken into a thread when, from 557's proposal; 3d, a turn that waits for a person set aside while
  the agent answers elsewhere, where the agent stands outside a turn its own as a whole (`Sleeping`
  with it), and `joined_thread_id` gone. Step 3a done: `Attendance` keeps a range per participant
  and thread; the threads an actor takes part in are shared by the channel and its runtime and
  changed by the channel alone (`ChannelActor::threads`); `TakeIntoThread` and `LetGoFromThread` -
  added, or joining by one's own act where one may read the channel; removed, or leaving as leaving
  the channel is asked - have the channel take the actor into the thread or let it go from it
  (`take_into_thread`, `let_go_from_thread`) and announce the join or the leave there; refused are a
  thread one takes part in already, the last thread one takes part in, a thread delegated into and
  an external agent's second thread (`Actor::takes_part_in_several_threads`). Opening a thread takes
  a founder who takes part in the channel into it as well. Its join or its leave of the thread
  changes where it takes part right where `run` announces it, and a leave reaches whoever left. One
  taken in hears the thread from its join on; an agent follows its threads as it takes its own join
  or leave in, in the order of what reaches it (`AgentBody::follow`): its branch of a thread it
  joined forks from where it stands in the thread's origin (`Transcript::fork`,
  `ActorRuntime::origin_of`), or begins at the root, and its part and branch of a thread it left go.
  Cancelling a thread ends the turns that run there alone (`Actor::cancel` of a thread). After a
  restart an actor starts in every open thread of its attendance; `joined_thread_id` is the line
  where one takes part there, else the first thread (`channel::joined_in`). Who takes part in the
  channel joins a further thread of it by their own act where they may read the channel: a thread
  asks nothing beyond it. REST takes a participant into a thread and lets them go from one, and lets
  one join and leave a thread by one's own act.
- **2026-11-22** — Step 3a committed. Decided by the maintainer: a thread knows the message it hangs
  at, and an agent's branch of it forks there, stable however far the branch of the thread it hangs
  at went on since - which settles 557's forked thread for a thread an agent is taken into later;
  joining a thread asks no permission of its own beyond the channel's; a leave reaching whoever
  left, and `joined_thread_id` changing as one is let go from the thread one joined in, stand. Done
  as a step of its own: `Transcript::fork` forks a branch up to and including the last entry of the
  thread's message (`ThreadOrigin`), or whole where the branch does not hold it - not taken in yet,
  and so nothing after it either; never heard, or replaced by a compaction, where what came after it
  is forked with it.
- **2026-11-22** — The step forking at a thread's message committed. Step 3b done: what a user says,
  nudges, asks to answer again and issues names its thread - `UserActor::send_message`,
  `request_response` and `execute_command` take one, the line where none is named, and so do the
  REST requests (`thread`); a request to answer a message again is made in the thread it was said in
  (`ChannelLoop::said_in`), one to answer a turn again in the thread the turn runs or ran in, which
  the channel knows (`ChannelLoop::retryable_turn`, `Actor::turn_under_way` with its thread); a
  command is issued in a thread (`CommandContext::thread`), where a `/nudge` asks and a `/workspace
  run` says its call, whom it names found among those who take part there. The channel admits a
  nudge, a request to answer again and a command of an agent only of whom takes part in the thread
  they name (`ChannelLoop::runs_in`), and ends the quiet of that thread alone; that thread's
  moderator routes a nudge, which so reaches whom it asks across threads. Decided by the maintainer:
  questions and approvals are asked and answered in their thread, whose context they are of -
  essential for questions; an approval may be told of everywhere -: a user answers questions in the
  thread they were asked in, which their actor keeps with them, and an approval in the thread its
  request was asked in, which the approver holding it tells (`ChannelLoop::approval_thread`); a
  change of a message, which is of the whole channel, happens in the line. A user no longer says
  anything in the thread they joined in: `joined_thread_id` is an agent's alone.
