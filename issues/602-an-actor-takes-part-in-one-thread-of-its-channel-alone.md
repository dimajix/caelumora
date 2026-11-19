# 602 — An actor takes part in one thread of its channel alone

**Status:** open
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

### Further points

- **After 593.** Step 3 changes the lifecycle procedures 593 builds - letting someone go holds the
  thread they take part in, a thread is closed once nobody takes part -; it waits for them.
- **A change of a message from before a fork** reaches every head whose path holds the message
  (557, *What is still hard*).
- **Stored data**: the agents' states and transcripts are kept per thread from step 1 on; what is
  stored has to be converted or deleted.

## To decide

- D7, the shape of a transcript with several threads.
- Who is taken into a thread, and when (557's proposal).
- The cut of the first user above, and the steps.
- Whether a nudge across threads is fixed before this issue - by routing it with the moderator of
  the thread of whom it asks -, or with step 3.

## Done when

- An agent takes part in several threads of its channel, with a branch of its transcript and a turn
  state for each, one turn at a time.
- A user writes in a thread they name, and a nudge reaches whom it asks in the thread it names.
- D7 and who is taken into a thread are decided and recorded here.

## History

- **2026-11-19** — Opened from 557's step 3, which 557 named and left without an issue, and from a
  nudge across threads found while reading how threads are routed.
