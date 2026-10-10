# The Agent Turn

How an agent answers: the cycle its `AgentActor` runs, the `Turn` it works on, how a turn survives a
crash, and how it can be told afterwards what the model saw. What an agent remembers beyond a turn -
notes, private notes, memory - is described in [MEMORY.md](MEMORY.md); the components that take part
in a turn in [COMPONENTS.md](COMPONENTS.md); tools and approvals in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

## The cognitive cycle

`AgentActor::run` is a state machine over `ActorState`. Two states wait between turns, five belong
to a turn.

**Between turns** (`Idle`, `Sleeping`) the agent waits for the next event, and takes it in together
with everything queued behind it - what came while it was busy - in order (`actor::intake::take_in`,
issue 222), then commits once:

*   **Perceive.** A `CompleteMessageGenerated` of another participant is appended to the agent's
    history - all that was offered to it, as it was said: what there is to hear, as the moderator
    forwards it, and what it catches up with alike (issue 258). What the agent hears of it is
    offered to its `PerceiveInProcessor`s: of what others said, their words (`Message::as_heard`,
    issue 224) - a `user` or `assistant` message with text, as a `user` message without its tool
    calls and results, which were the sender's; nothing of a tool message, an empty one or a
    `system` one. The
    rule is the same live, for what was queued and for what an agent catches up with on joining,
    which comes from the channel's history as it was said. The processors may drop, change or add
    messages; what passes is what the agent processes, and is appended to its transcript - whatever
    the personality configures (issue 196). A failing stage is logged; what was heard stays in the
    history and is not offered again. An external agent takes in what it hears the same way
    (`actor::intake::perceive`, issue 206). No turn begins, and nothing is asked of the policy
    (issues 85, 90).
*   **Begin.** The last `ResponseRequested` from the moderator among what was taken in begins one
    turn on its state (`AgentChannelState::begin_turn`, see below), announces `ResponseStarted`
    with the turn's id and has the agent's `ContextProvider` assemble the context. The agent goes
    on in `Planning`. The requests before it are dropped: the one turn answers the conversation as
    it stands, having heard every message, and acts in the contexts of the last request - with the
    authority of the last who asked, never of someone who did not. An agent that falls behind thus
    answers once when it is free again, not once per request that waited, each to a conversation
    that has moved on. A sleeping agent ignores the request; only `wakeup` ends the sleep. Whether
    it sleeps is read once everything is taken in: the channel wakes an agent before it delivers
    the message that asks it, and the request may come in the same batch.
*   **Answer again.** The requests to answer again among what was taken in are answered (issues
    234, 235, see [CHANNELS.md](CHANNELS.md#answering-again)). The one of the turn the agent gave up
    for it is accepted: its answer is gone already. Otherwise the last one of a message is judged:
    if the message is the agent's last answer (`AgentChannelState::retried`), it is taken back with
    the rest of its turn; otherwise the agent rejects it. Every other request to answer again
    in the batch is rejected. A retry accepted begins a turn as above, given the transcript as it
    stands - one turn together with a request to respond in the same batch, in the request's
    contexts.

**In a turn** the agent moves through these states:

*   **`Planning`** calls the model with the context and the agent's tools and streams the answer.
    Partial messages are published as they arrive. When the complete message contains tool calls,
    the agent registers them for approval where their tool requires it and goes to `ExecutingTools`;
    otherwise to `Reflecting`. A turn makes at most 25 model calls (`MAX_MODEL_CALLS`), then it goes
    to `Reflecting`. The count (`TurnState::model_calls`, `Turn::count_model_call`) is stored with
    the turn, so a turn resumed after a restart goes on counting.
*   **`ExecutingTools`** runs the calls that are ready. Once every call of the step has finished,
    the tool message goes into the turn and the agent returns to `Planning` - or to `Reflecting` if
    a tool asked for the turn to end, as the stop tool does. If calls still wait for a person, it
    goes to `WaitingForUser`. A cancel or stop interrupts the call under way - it is answered as
    interrupted - and starts no further one (issue 161).
*   **`WaitingForUser`** waits for a person: for `ApprovalGranted`, `ApprovalDenied` or
    `ApprovalTimedOut`, or for `QuestionAnswered` or `QuestionDeclined` (issue 165). It records
    what they settle and returns to `ExecutingTools`; an answer that does not count is left out,
    and it waits on. Other events stay queued until the turn is over. See
    [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md) for approvals and questions.
*   **`Reflecting`** takes the turn over (`Turn::commit`), stores the state and announces the
    end with `TurnFinished`. The agent then lands in the state a tool requested (`Sleeping`
    after `stop` or `bye`) or in `Idle`.
*   **`Cancelling`** is entered when the channel cancels the turn. It takes the turn over like
    `Reflecting` - so observers learn that generation stopped - and returns to `Idle`. Calls of the
    step still without a result - waiting for an approval, or approved but not yet run - are
    answered as not executed because the turn was cancelled, so that the transcript holds no call
    without an answer (issue 148).

Every message that leaves the agent passes its `EmitOutProcessor`s first, of which there is no
implementation yet (issue 99), and is published through `Turn::send_event`, which leaves out what
only the agent's owner may see (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)). A complete message
is *said* (`Turn::say`, issue 258): kept in the turn, then published - the model's message, the
tool message of a step, and the tool message that answers the calls a turn abandons as it ends,
so that the channel learns of every message the turn keeps as it is made. A partial one is only
shown (`Turn::show`). Both take the actor's output as a parameter: the turn is handed to every
component working in it, and none of them is to speak in the agent's name.

When a plan is in progress (`create_plan`), the messages of its steps are *internal*: they are
published as intermediate messages and kept like every message - in the transcript, the agent's
history and the channel's (issue 258) - but no other participant hears them, and they are not the
answer. The answer after the last step is the final one. The instruction for each step, and the one
to write the final answer, are prompt parts placed among the turn's messages, not messages (see
[Instructions within a turn](#instructions-within-a-turn)).

An error in one of the turn's states ends the turn, not the agent: the agent logs it, gives the turn
up (`Turn::give_up`), emits `TurnGivenUp` with the reason `Failed` and returns to `Idle` (see
[CODING-GUIDELINES.md](CODING-GUIDELINES.md)).

**Where it stands is announced** (issue 243): the agent tells the users its state as it changes,
with `StateChanged` - once as its run starts, and at the top of every step what the steps before
moved it to (`RunControl::unannounced`), so that each state is told once - `Planning` right after
the turn's `ResponseStarted`, which the step that takes the request in sends. It speaks in its own
contexts (`Answering::own`), not a turn's: its state is the agent's, not part of an answer. One move
is not the run loop's: the channel wakes a sleeping agent itself, synchronously, because the
moderator, deciding right after whom to ask, leaves the sleeping out (`RunControl::wakeup`). So
`wakeup` also notifies the run loop, which waits between turns for the next message *or* that
notice, and on the notice announces `Idle` and waits on - after `/wakeup`, no message follows. An
external agent announces `Planning` and `Idle` as its answer begins and ends.

## The `Turn`

The agent's state in its channel is its `AgentChannelState`: its `MessageHistory`, its `Transcript`,
its notes and the texts of its prompt parts. Its own state across channels is its `AgentState`. A
step of the cycle works on a `Turn` (`caelumora_runtime::agent::turn`), the handle to both. Its
module holds the handle and what components work with in `mod.rs`, and the rest of a turn's logic
in a file per subject: `ending.rs` takes a turn over or gives it up, `context.rs` builds what the
model is given, `plan.rs` moves the plan on, `tool_calls.rs` keeps the books of a step's tool calls.

What a turn does to the agent's state when it begins, says something and ends is a rule of that
state, kept with it in the model: `AgentChannelState::begin_turn`, `said` - a message into the
transcript and the history as it is said (issue 260) - `finish_turn` - its notes, its end - and
`give_up_turn`, which records why the turn ended. The working state itself, `TurnState`, is in
`caelumora_model::turn`. `Turn` calls these and does what runs around them: abandoning tool calls,
applying the agent's memory.

The working state of a turn is one value, `AgentChannelState::turn: Option<TurnState>`, which is
`None` between turns (issue 85). It holds what the turn is and what it has done so far:

*   which turn it is (`TurnId`, issue 235),
*   the envelope it answers (`EventContext`),
*   whose authority it acts with (`SecurityContext`),
*   the classification of its channel as the turn began - where the channel is, the turn reads from
    the envelope it answers - which bounds what it may read,
*   its label: the join of the classifications of everything it has seen so far, at most the
    channel's, which everything it writes is classified as (issue 168, see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-a-turn-writes-as)),
*   what its next message is produced from (see [The turn's context and the
    resolver](#the-turns-context-and-the-resolver)) - what it said is in the transcript and the
    history already, each message with what it was produced from (issue 260),
*   the changes staged to the agent's states,
*   the tool calls, the plan and the state a tool requested.

The id, the envelope, the authority, the classification, the label and the context of the next
message are the turn's `Work` (`caelumora_model::work`, `TurnState::work`): what any piece of work
of an agent is, a turn as much as an external agent's exchange with its remote agent (issue 204, see
[CHANNELS.md](CHANNELS.md)). The rest is the turn's own.

The id is given when the work begins and stored with it, so that a turn resumed after a restart is
the same turn; work stored before it was kept gets a new one as it is loaded. `ResponseStarted`
announces it, and every message the work produces carries it (`Message::turn_id`), so that which
messages of an agent belong to one turn can be told from the messages alone - by a client that loads
a channel during a turn as much as from a history. `AgentChannelState::said` sets it on everything
the turn says, whoever built the message; what is only shown, a partial message, gets it where the
agent makes the model's message its own (`stamp`); an external agent sets it on what it streams and
answers. It is not covered by the digest, like a message's time, and a modification does not take it
over: a changed message stays in the turn it was produced in. The agent's `RunControl` knows the
turn under way by its id as well, the one place reachable while the run loop holds the state.

Work between turns does not touch it: perceiving a message only appends to the transcript and the
history. Between turns the `AgentActor` works on its `AgentChannelState` itself; a `Turn` is had
only while one is under way (`Turn::of`, `None` between turns), and taking it over or giving it up
uses it up, which leaves the state to be stored.

Beside its state, a turn reaches the agent's session: `Turn::session()` answers the `SessionView` of
the turn's views (issue 163), from which the components read the other channels as the
confidentiality rules allow - at the classification the turn began with (see
[MEMORY.md](MEMORY.md#reading-other-channels)). A turn outside of a channel, in tests, has none and
reads nothing.

### Beginning and ending a turn

*   **`AgentChannelState::begin_turn`** begins a turn and fixes the first four of the values above.
    The classification and the publication are taken once and hold for the whole turn, so that what
    the turn reads, publishes and remembers is judged alike, even if the channel's level changes
    meanwhile. All four are stored with the channel state, so that a turn resumed after a restart is
    the same turn.
*   **`Turn::commit`** takes a completed turn over:
    *   its messages are in the transcript and the history since they were said - internal steps
        too (issue 258), each in the history with a `ContextRef` (see below); it reads its answer
        from the transcript (`Transcript::of_turn`) and records the turn's end, `TurnFinished`, in
        both;
    *   it applies the changes staged to the agent's state in the channel, and those staged to its
        own state;
    *   it clears the working state, so that none of it is stored with the channel state.

    It returns a `CommittedTurn`: the final messages for `TurnFinished`, whether the
    channel state changed, the contexts the turn's last events are sent in, and the tool calls it
    abandoned.
*   **`Turn::give_up`** gives up a turn that failed, was stopped or asked for again (issues 145,
    235). Nothing it said is unsaid (issue 258):
    *   its messages stay where they were said, on the active path of the transcript and the
        history, each with its `ContextRef`;
    *   its end follows in both, `TurnGivenUp` (issue 197): the reason (failed, with what its users
        were told; lowered, with the channel's new classification; stopped; retried), the time and
        which turn it was, whose messages before it carry its id (issue 241). A turn that said
        nothing leaves its end too. The digest covers the turn and the reason, not the time;
    *   nothing it staged is applied - no note, nothing remembered;
    *   what its tools did stays done, and its calls and their results stand in its messages. A
        step whose calls do not all have their result is ended first, as on a cancel: those without
        one are answered as not executed, and that answer is said (issue 148). The model is told
        why the turn ended where its end stands - the `ContextResolver` words it, a fixed text that
        says nothing of the conversation. A turn that no longer flows into its channel - the
        channel was lowered below it (issue 141) - is recorded as `Lowered` whatever ended it, and
        of it the model is given only which tools it called and how they ended
        (`Message::names_only`), classified as the channel is now: what it read and said after the
        lowering, or in a step it did not complete, was never released.

    `cancel` does not give a turn up; it takes it over - unless the turn no longer flows into its
    channel. A turn whose
    answer is asked for again while under way is given up (`Retried`, issue 235): the run loop
    takes `Interrupt::Retry` after the step it cut short, as it takes a stop, gives the turn up and
    goes idle, and the request on its way begins the next turn. Its approval requests are withdrawn
    as for a turn that failed - a stopped agent's too, though the approvers discard them as it
    leaves anyway.

Before every step of a turn, the agent asks whether the turn still flows into its channel as it is
now (`ActionType::Publish` of the turn's classification, in the agent's name). Once the channel was
lowered below it, the turn is given up as `Lowered`, whatever it was about to do - run a tool, take
the turn over after a cancel - and the users are told with `TurnGivenUp` (issue 141, see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

**A message given** to the agent to read in a turn - a skill a member loaded for it with `/skill`
(issue [22](../issues/22-skill-management.md)) - is recorded by `Turn::give`
(`AgentChannelState::given`) with the knowledge it is: into the transcript as a `Given` entry, which
keeps what it is (`KnowledgeSource`), so that what the agent knows can be told from its transcript
(issue [16](../issues/16-context-compression.md)); into the agent's history without a context,
since the agent did not produce it; the knowledge recorded with the turn, and the turn's context
moved on past it. It is not published, and the turn has seen it. Its digest is the message's, as if
it were a `Message` entry: the model is given the same, and what it is stays outside the digest.

**A command turn** (issue [269](../issues/269-agent-commands.md)) carries out a command a user
asked the agent for, rather than answering: `AgentChannelState::begin_command_turn` begins it with
no tools and the command kept in `TurnState::command`, and the component that carries the command
out stages on it as a tool does. Taken over, it records the command before its end
(`HistoryEntry::CommandExecuted`, with whom it was asked by); not carried out, it is dropped
without an end - nothing was said or applied. It announces neither its start nor its end, is not
the turn under way that a cancel or a retry reaches, and is never stored under way: one found
after a restart is dropped. See
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#commands-of-an-agent).

Both use the turn up and hand back the contexts it had as an `Answering` (`crate::actor`), which
sends the turn's last event - `TurnFinished`, or `TurnGivenUp { turn_id, reason }` for
every reason but a stop, the agent having left (issue 258) - with the publication the turn had, and
withdraws the approval requests of the calls it abandoned. An external agent publishes
its answers through the same `Answering`.

### Storing and resuming

The state is stored after every step, together with the state that comes next (issue 85). What a
crash leaves in the store is the step still to do, not the one that is done. What a step writes is
what changed (issue 193, see [STORAGE.md](STORAGE.md#what-a-state-tells-a-store)): the messages and
notes appended since the last step - the turn's own messages among them, written once, as it said
them (issue 260) - the prompt texts and tool sets kept since, and the small parts whole - the
processing state and the turn under way, which holds no message and is changed in place. A step
between turns, or one that only moved the actor on, writes the small parts alone. When the agent is
loaded again, the turn is resumed (issue 75):

*   `Planning` asks the model again with the turn so far.
*   `Reflecting` takes the finished turn over.
*   `WaitingForUser` keeps waiting.
*   `Cancelling` ends the turn.
*   `ExecutingTools` is the exception. A tool may have run without its result being stored, and
    running it again would repeat its effect (issue 20). So every call that was ready to run is
    answered as *not executed* (`prepare_resume` in `actor/agent/mod.rs`), the tool message is
    built from that, and the model decides again.

A turn state stored without the turn's working state cannot be resumed; the agent starts `Idle`.

A deliberate end never leaves a turn in the store. `cancel` and the stop tool commit the turn. An
agent removed from its channel is told to `stop`: it gives up a turn under way, tells the channel
so (`TurnGivenUp`, `Stopped`), stores itself idle and ends its task - the channel takes what it
says until then (issue 262), and announces its removal once it has ended (`ParticipantRemoved`,
see [CHANNELS.md](CHANNELS.md)). The channel aborts it only if it does not end - a tool that
hangs, say.

## What an agent saw

For every message an agent produced, it can be told what its model saw when it wrote it (issue 06).

### The transcript as a chain

The `Transcript` is the sequence of messages the agent's context is built from: its own messages in
full, with tool calls and internal steps, and what others said as it received them. It is kept as a
chain. Every entry has a `Digest` (SHA-256, `caelumora_model::digest`) over its message and the
digest before it, so one digest names the whole transcript up to that point by content.

What goes into a message's digest is fixed in one place, `digest::message_chain`, behind a version
tag:

*   included: id, role, sender, content, reasoning, tool calls and results, and whether the message
    is internal;
*   not included: timestamps, usage statistics, resolved mentions and metadata.

JSON values such as tool arguments are hashed with sorted keys, so that the digest depends neither
on how a value was built nor on `serde_json` features.

The chain is one generic type, `caelumora_model::chain::Chain<E>` (issue 48): entries that are only
appended to, each in a `Node` with the digest its content's scheme computed over it and the
digest before it, with lookup of the prefix an earlier digest names. It is stored as its nodes
and its head, `Stored<E>`, which is also what it tells a store of what changed since (see
[STORAGE.md](STORAGE.md#what-a-state-tells-a-store)). A message does not name its predecessor: the
same message is in several chains with a different predecessor in each, so the predecessor
belongs to the chain, stored as `parent`, the predecessor's digest, beside the entry
([117](../issues/117-message-prev-id.md)).

Every content is one entry type, an enum of what can happen, with its methods as inherent methods
on its chain ([197](../issues/197-one-chain-one-model-of-change.md)); what stands is read from
the entries:

*   `Transcript` is `Chain<TranscriptEntry>`: a `Message` the model was given or produced, a
    message `Given` to the agent to read with what it is (`KnowledgeSource`, issue 16), a message
    `Modified`, `Revoked` or `Reclassified`, or the end of a turn, `TurnFinished` or
    `TurnGivenUp` (issue 258). `messages` is the messages of the active path as they stand, what
    the model is given.
*   `MessageHistory` is `Chain<HistoryEntry>`: a message `Said`, with what it was produced from
    for the agent's own, a message `Reclassified`, `Modified` or `Revoked`, the channel's
    classification `Fitted`, the start of a step of a turn's plan, `PlanStepStarted` (issue 263,
    recorded by the channel only), the end of a turn, `TurnFinished` or `TurnGivenUp`, a command
    an agent carried out, `CommandExecuted` (issue 269), and a compaction of its context,
    `Compacted` (issue 16, the agent's history only). `messages` and `message(id)` fold every
    branch in the order things happened into the messages as they stand, so a reclassification, a
    modification or a revocation applies to a message on any branch, as it did in the channel.
    `items` reads the active path as one list: each message said as it stands, where the steps of
    plans began, how the turns ended, the commands carried out and the compactions - what a reader
    of the history is shown (issue 258).
*   Notes and memory are `Chain<LedgerEntry<K>>`, a ledger of changes to numbered items (see
    [MEMORY.md](MEMORY.md)).

Messages share the `message_chain` digest, and a change to a message the same scheme in both chains
(`message_modified`, `message_revoked`, `message_reclassified`); the other entries have schemes of
their own (`crate::digest`), so that every digest recorded before names what it named.

A chain is a tree with a head ([08](../issues/08-retry-and-edit-past-answers.md)):

*   `push_with` appends after the head and makes the new entry the head. `move_head` sets the head
    back to any entry, or to the root; what follows it stays, as a branch no longer active. Nothing
    is removed.
*   What the chain says - `digest`, `entries`, `last`, `len` - is the *active path*, from the root
    up to the head: the conversation as it stands. `all_entries` are those of every branch, for what
    has to consider all of them: looking up a message by id, reading the history's messages as they
    stand with every reclassification recorded since, the next number of a note.
*   `prefix` finds an entry on any branch, so a `ContextRef` into a branch no longer active still
    resolves.
*   A branch no longer active is what an external agent's transcript leaves: it starts anew with
    every call (`Transcript::start_anew`, issue 205), since the remote agent keeps what it was
    given, so the active path holds only what it is given next, and every call before is a branch
    from the root - what did happen, one call each. A turn given up no longer branches (issue
    258).
*   `children` are the siblings among which a branch is chosen, and `leaf_from` finds where a
    switch to a branch goes on: at the newest child, unless the entry remembers another as last on
    the active path - which it does only once the head went back to an older one, so that an entry
    appended changes nothing before it.
*   An entry pushed with the digest of one already there - the same predecessor, the same content -
    is that entry: the head moves to it.
*   Nothing changes an entry after it was added. Append and move the head are the only changes,
    and the chain keeps what changed since it was last stored (`changes`, `mark_stored`, `apply`;
    [193](../issues/193-external-state-rewritten-whole.md)).

Neither ending a turn ([258](../issues/258-what-was-said-is-lost.md), see [Beginning and ending a
turn](#beginning-and-ending-a-turn)) nor changing a message moves a head: each is recorded after
what it is about (see below).

### Changes to a message

A message once said can be modified or taken back by its author
([234](../issues/234-modify-revoke-and-retry-messages.md)). Neither rewrites it: the change is an
entry of its own after it - `Modified` with the message as it reads from now on, or `Revoked` - in
every chain that holds the message (`modify` and `revoke` of `MessageHistory` and `Transcript`, both
called where an agent takes a change in, `actor::intake::take_in`). A chain that does not hold the
message records nothing, and so does one where the message as it stands cannot be changed so: taken
back already, or modified by another sender or in another role.

A message reclassified ([133](../issues/133-classification-model.md),
[239](../issues/239-reclassified-message-not-in-agents-state.md)) is recorded so as well:
`Reclassified` in both chains, where an agent takes it in (`take_in`). A turn begun
afterwards has seen the message as it stands (`begin_turn`), so a message released no longer raises
the label of what follows.

What stands is read from the entries, by one rule, kept with the message (`Message::modify`,
`Message::revoke`, `Message::reclassify`):

*   a modified message keeps its place, its id and the time it was said, and takes over what the
    change says - content, reasoning, classification, mentions - and when it was made
    (`Message::modified`). Its tool calls and results stay those it was said with, so that a call
    and its result cannot come apart;
*   a message taken back keeps its place and loses what it said - content, reasoning, tool calls
    and results (`Message::revoked`). The history keeps it so, for whatever names it by id; the
    model is no longer given it;
*   a message reclassified takes over the classification and nothing else; one taken back is not
    reclassified, since it says nothing that could be released.

The transcript's messages are read in one place, `transcript::as_they_stand`: every entry of a path
with its message as it stands at the end of that path. `messages` and the `ContextResolver` read
through it. Since a change is applied only within the path that holds it, a
`ContextRef` taken before the change resolves to the message as the model saw it then, and one
taken after to what stands. A prompt part placed after a message taken back stays where it was: the
entry is still there. What is about every entry ever added - the tokens used, whether a message
reached the transcript at all - reads `all_entries` instead.

An external agent's transcript starts anew with every call (issue 205), so a message its remote
agent was given is on a branch off the active path, where `modify` and `revoke` record nothing. The
external agent tells its remote agent of the change instead
([236](../issues/236-external-agent-not-told-of-changes.md)): a change the transcript did not take,
`take_in` hands to the actor where it came - so in order with what was said - and the external
agent, while it keeps a remote conversation that was given the message, appends a *notice* to its
transcript, given with the next call as any message is. An agent run here tells nobody. A notice is
a message of its own (`Message::notice_of` names the one it tells of), in the
name and role of the message's sender, so that it is marked as every message is; it names the
message by what it said as it was first given (`Transcript::message`), shortened, and says what
became of it: `[changed an earlier message "..."; it now reads:] ...` or `[took back an earlier
message "..."; it no longer stands.]`. A notice not given yet is modified rather than followed by a
second one, so the remote agent is told once, of the message as it stands. A notice is never in a
history. The transcript records it word for word, so what a call was given is
resolved as it was. A reclassification is not told; the remote agent knows no labels. A notice only
tells: the remote agent decides what to make of it, and nothing in its wording asks it to disregard
something, which a filter of the remote side may take for an injection.

### What is recorded

`Turn::commit` records a `ContextRef` with every message it appends to the history:

*   the digest of the transcript before the message,
*   the turn's prompt parts, each as a `PromptRef`,
*   the tools the model was offered, as the digest of the set (`ContextRef::tools`, see [Tools
    offered](#tools-offered)),
*   the knowledge read before the message (`ContextRef::knowledge`, see [MEMORY.md](MEMORY.md)),
*   how the model was given its context: its profile (`ContextRef::profile`) - see [Reasoning
    given back](#reasoning-given-back).

Which of the transcript's messages were the turn's own is not part of the reference: they carry the
turn's id (`Message::turn_id`), and the turn is that of the message the reference is recorded with
(issue 241).

`AgentChannelState::recorded_context_of(message)` finds such a reference, with the message's turn,
and the `ContextResolver` resolves it (see [The turn's context and the
resolver](#the-turns-context-and-the-resolver)) to a `ResolvedContext`: the messages and prompt
parts the model saw, in the order it was given them, each message with its reasoning only where the
model was given it, and the tools it was offered. The channel's history identifies the agent by the
message's sender.

### Tools offered

A turn offers its model the agent's tools as they were when it began: they are asked for once, in
their tool sets (`ToolExecutor::tool_sets`, issue 194), in `AgentActor::request_response`,
and fixed in the turn (`TurnState::tools`, flat, issue 137) like its classification and publication
- a resumed turn keeps them. The model is told of a tool only its name, description and parameters -
the backend sends no more (`LLMClient::chat_completion`); whether its calls need approval, who sees
them and how its results are classified stay with the runtime, which labels the model's answer
(`label` in `actor/agent/mod.rs`) and puts its calls up for approval (`ToolExecutor::admit`)
by the same `Tool`s. A tool that changes or disappears during a turn is offered as it is from the
next turn on; a call of one that is gone fails as unknown.

Beginning a turn (`AgentChannelState::begin_turn`) keeps the tool sets - each tool whole, with how
the runtime treated its calls then - in `AgentChannelState::offered_tools`, once per digest
(`digest::tool_sets`, in the order offered, over all of it: the same tools treated otherwise are
another offer), and puts its digest into the turn's context, so that every message of the turn
records it; no tool sets are recorded as `None`. An offer recorded before issue 194, a flat list of
what the model was told of each tool, is read as one set without a name, its tools needing no
approval as far as is known.
`LLMClient::chat_completion` gets the tools from the resolved context, like the messages - the
tools of all sets in one list, since the backends know no sets - so what the model was offered and
what is recorded as offered are the same.

### Reasoning given back

Which reasoning of earlier messages a model is given again is a property of the model (issue 63):
some must not see earlier reasoning, others need the reasoning that led to a tool call within the
same turn. The agent's `LLMClient` declares it in its `ModelProfile`, as a `ReasoningHistory` (both
`caelumora_model::context`):

| Rule | The model is given |
|---|---|
| `never` | no reasoning at all |
| `current_turn` | the agent's own reasoning of the turn under way - the default |
| `always` | the agent's own reasoning of every turn |

Whatever the rule, a model is never given another actor's reasoning.

The profile is the agent's model's, handed to `AgentChannelState::begin_turn` and fixed in the
context of a turn when it begins - so that a resumed turn keeps the profile it began
with - and applied in one place: the `ContextResolver`, which resolves what the model is given - the
transcript before the turn, then the messages the turn produced (`ReasoningHistory::given`). A
backend therefore gets only what its model may see and implements no rule of its own. The stored
transcript keeps all reasoning; only the model's input leaves it out.

Since what the model was given is less than the transcript holds, the `ContextRef` records the
profile, the turn's messages carry its id, and resolving it applies the same rule again. The digests
are unchanged: they name the stored transcript, not a filtered copy of it.

### Who said what

What other participants say reaches an agent as `user` messages (`as_user_messages`), and a model's
input has no place for their sender: Ollama drops a `name`, A2A knows only the roles `user` and
`agent`. So the sender is marked in the content, as the profile says (issue 135,
`ModelProfile::sender_format` and `sender_style`):

| `sender_format` | A message of another participant is given as |
|---|---|
| `none` | its content alone |
| `prefix` - the default | `@alice: text` |
| `xml` | `<message from="@alice">text</message>` - only `<message` and `</message` in the content escaped |

`sender_style` names the sender by `handle` - the default; derived from the name the message holds,
so that a stored transcript reads the same after a renaming - or by `name`. Only messages a user or
another agent said to the agent are marked, never the agent's own, a tool's or the system's: a model
whose own messages carry the marker imitates it and continues the conversation as a transcript
(`work/sender_names/RESULTS.md`). The rule is applied where reasoning is
(`ModelProfile::given`, by the `ContextResolver`), so the context panel shows the markers, and the
stored transcript keeps the message as received. A context recorded before the rule existed reads as
`none`, as it was given.

How the others' messages are marked is told with the roster
([who is in the channel](#who-is-in-the-channel)): `ModelProfile::sender_explanation` words it -
for `prefix` and `handle`: "Messages of the others begin with their sender's @handle and a colon:
"@handle: ...". Your own messages carry no such marker: answer without one, and never write a
message for someone else." Without such an explanation models imitate the marker.

A marker is a claim like any text: a participant can write another's into their own message, and
models believe it (issue [150](../issues/150-impersonation-towards-the-agent.md)). Nothing with
consequences may rest on whom the model believes said something.

An external agent is given what was said by the same rule (`ExternalAgent::profile`): its actor
records the profile in the context of its call, so that the context of its answer shows what the
remote agent was given (see [CHANNELS.md](CHANNELS.md)).

### Tokens used

Every message the model produces carries the tokens of the call that produced it - of its prompt and
its answer - and the size of the context the call had (`Message::context_size`, issue 209, see
[CONFIGURATION.md](CONFIGURATION.md#the-model)). What an agent's calls in a channel used is summed
from its transcript when it is read (`AgentChannelState::usage`, a `TokenUsage`), not kept as a
counter: over the agent's own messages only - the others' in its transcript carry their senders'
tokens - on every branch, since their calls were made, the turn under way's included, whose messages
are in the transcript as they are said. Two groups come out of it and mean different things:

*   **context** - the input and output tokens of the last call (`last_call_tokens`) against its
    context size: how full the agent's context was at its fullest in that call, its reasoning
    included, which a later call may not be given back;
*   **usage** - the total input and output tokens of all calls (`prompt_tokens`,
    `completion_tokens`): what was processed. Every call is given the whole context again, so the
    totals grow much faster than the context and are no measure of how full it is.

An external agent's calls report no tokens, so its usage is unknown, not zero. The views hand it out
per member, in its snapshot with `Detail::USAGE`, to whoever reads the channel - who sees the tokens
of every published message anyway.

### Digests are stored, not recomputed

A digest is a record of what was sent to the model, not a value to derive again. It is stored with
its entry and never recomputed. Were the scheme to change, recomputing would silently change every
digest the history refers to; stored, old entries keep their digests and only new ones get the new
scheme's.

### Compaction

This relies on nothing ever being removed from the transcript, so that an earlier digest names a
prefix of one of its branches. A turn under way changes nothing before its head either: what it
says is appended as it says it (issue 260, see [The turn's context and the
resolver](#the-turns-context-and-the-resolver)).

A compaction ([16](../issues/16-context-compression.md)) produces a new context that came from the
old one but does not continue it, and is built from what the chain already is - a tree
(`AgentChannelState::compact`, used through `Turn::compact`):

*   **A branch of its own.** The transcript begins anew from the root (`start_anew`, as an external
    agent's does before every call), with what replaces the older messages - a summary, the skills
    given again as `Given` entries with the knowledge they are - and then the entries kept, copied
    from the active path as they stand: a change of a message is in its copy, a message taken back
    is left out, a turn's end is copied as it is. Every copy gets a digest of its own, since what
    precedes it is another: the new context is another context.
*   **The old branch stays.** Every `ContextRef` recorded before names it and resolves through
    `prefix` to what it did; the `ContextResolver` does not know there was a compaction. Only
    what the turn says afterwards is produced from - and recorded with - the new branch: the turn's
    context moves to its head. Prompt parts the turn placed after an entry name the old branch and
    are dropped; who placed them places them again.
*   **Where it came from** is recorded in the agent's history, `HistoryEntry::Compacted { from, to
    }` - the heads of the transcript before and after -, followed by what replaced the older
    messages, as messages given to the agent. It is a relation of its own, not the predecessor link
    of the chain, and - since the model is not given it - in no digest of the transcript: two
    identical contexts get the same digest however they came about.
*   **Counted once.** A message kept is in the transcript twice, once on each branch; the tokens
    used (`AgentChannelState::usage`) count it where it was first recorded.

What replaces the older messages, where the cut is made and when a compaction happens is not the
state's: the mechanism takes the replacement and the first entry kept, and components decide. When
is the `compaction` component's, which carries out `/compact` (see
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#built-in-tools-and-commands)); how is the agent's
`CompactionStrategy`'s, of which it has exactly one (`crate::compaction`). The strategy compacts
through `Turn::compact` and says what it did (`Compaction`), or that there is nothing to compact,
or why it failed (`CompactionError`); the command tells the channel.

The default strategy, `summary_compaction`, keeps at least the newest `keep_last` messages, from the
start of the turn the oldest of them belongs to, so that no turn is cut apart; has the agent's own
model summarise the rest, given as every call is (`Turn::context_until`); and gives the skills
loaded in that rest again. The summary is a message of the agent in the role `user`, titled
"Summary of the conversation so far", classified as the turn's label, and carries the tokens of its
call. Compacting on the agent's own is [284](../issues/284-automatic-context-compaction.md); it asks
the same strategy.

## Prompt parts

What the model is given besides the conversation - the system prompt, notes, what the agent
remembers - is a *prompt part* (issue 47). The `ContextProvider` decides where the parts go; the
`Turn` records what it was handed.

Every part the runtime writes has one form (issue 249): a short title in brackets on a line of its
own - `[Who is in this channel]`, `[Your private notes]`, `[Planning]` - then what needs explaining
as plain text, then the content. `context::titled` makes it, and `render_sections` for a part that
lists entries; a part that says nothing is no part. The parts are joined by a blank line, and an
external agent is given each on its own, so the titles keep them apart either way. The text of
`static_system_prompt_provider` is the personality's and stays as written, under the title its
parameter `title` names, if any.

### The `ContextProvider`

An agent has exactly one `ContextProvider` (issue 98), a facet registered with
`ComponentRegistry::single`. A personality may configure its own, to try another way of assembling
the context; otherwise it gets the default, `turn_context_provider`, or the one
`AppConfig::component_defaults` names. Two would each answer for the whole context, so a
personality with two is refused.

A `ContextProvider` only answers (issue 214): given the turn to read as an `AgentWork`
(`current_context`), it returns the prompt parts, each where it goes, and the knowledge they hold.
The `AgentActor` sets both at once with `Turn::set_context(parts, knowledge)` (issue
[285](../issues/285-set-context-with-knowledge.md)), nothing else; that a provider does nothing else
to the turn the type ensures, since an `AgentWork` only reads. What the turn has seen of it - the
knowledge recorded, every part's classification - is one rule of the work,
`Work::take_in_context`, which an external agent's work follows as well for the parts it gives its
remote agent and those it gave before.

The `ContextProvider` runs once, at the start of a turn, so a part stays in place across the tool
loop. When it fails, the turn fails, whatever its fail policy: without a context there is nothing to
give the model.

### Parts and their position

A `PromptProvider` contributes a part with a `PromptPosition`:

*   **`Start`** - the system prompt. It goes before everything, as a system message.
*   **`BeforeTurnInput`** - goes transiently before the newest message of the turn, as a user
    message; at the end, if the agent wrote that message itself, in a turn without new input.

The distinction exists because many models cache the prompt up to the first change, and some merge
every system message into the system prompt at the front. Information that changes often therefore
does not belong in the system prompt. A part is never stored in the transcript, which stays a stable
prefix.

`TurnContextProvider` asks for every `PromptProvider` of the agent by type (issue 99) and joins
their parts per position, in the order of the configuration. `static_system_prompt_provider` takes
its position from its parameter `position` (`start` by default, or `before_turn_input`), and how its
prompt is classified from `classification` (`public` by default). Every part is classified as its
provider says - a part that does not say counts as the channel - and what the parts of one position
are joined into, as the join of theirs; the turn has seen it (issue 168).

The parts may be left out, each by the fail policy of its own component
(`ComponentImpl::apply_fail_policy`, which the message processors' stages use too): a part that
fails open is left out of that turn with a warning; one that fails closed fails the turn.

A `PromptProvider` is not given the `Turn` but what it reads of it, an `AgentWork` (`Turn::work`,
issue 204): the agent, the contexts the turn is done in, the classification it began with and its
label, the agent's state in the channel and its own state, and the session to navigate. That is the
part of a turn that every piece of work of an agent has - an external agent's exchange with its
remote agent as well (`caelumora_model::work::Work`) - so that the same providers can serve both.
What only reads an agent's notes or memory (`AgentNotes::current`, `AgentMemory::recalled`) takes
an `AgentWork` too; what stages changes takes the `Turn`.

### Who is in the channel

An agent is told who it is and who else is in its channel by the component
`channel_members_prompt_provider` (issue 176), which a personality wires in next to its static
system prompt. It reads the channel's members through the turn's views (`ChannelView::members`) at
the start of every turn, so it follows joins and leaves, and lists

*   the agent itself, by its name and `@` handle as the channel knows them, with its personality's
    introduction - so a personality's prompt should not name its agents: two agents of one
    personality have different names;
*   every other current member that is a user or an agent, by name and handle, with their
    introduction (`MemberView::introduction`). Approvers, former members and participants that no
    longer exist are left out.

An agent's introduction is its personality's (`Personality::introduction`), read through
`Agent::personality_id` like its components; a user's is the one they wrote in their `UserProfile`.
A person's introduction is marked as written by them ("In their own words: ..."): the system prompt
is what models trust most, and a participant's text about themselves must not read as an instruction
of the system (issue 150). Each introduction is put on one line, so that it cannot break out of its
entry. Names and introductions are public, and so is the part.

It goes into the system prompt by default (`position`): the roster changes rarely, and a change
ought to reach the model at once, at the cost of the prompt cache from there on. After the others it
tells how their messages are marked ([who said what](#who-said-what)), as the profile of the work
words it (`AgentWork::profile`) - nothing where nothing is marked or nobody else is there.

An external agent is told the same roster (issue 204, see [CHANNELS.md](CHANNELS.md)), with the
parameter `changes_only`: its remote agent keeps its conversation, so it is told the roster when it
is first asked and then only when it changed - the whole of it again, followed by who joined and who
left since it was last told ("Since you were last told who is here: - Joined: ..."), told apart by
who they are, not by name. The component remembers what it told while it lives - it is built for
each actor, so for each channel - and tells all of it again after a restart. For an agent run here
the parameter makes no sense: its system prompt is built anew with every turn.

### The channel's language

A channel's conversation may be held in a language (`Channel::language`, issue 56, see
[CHANNELS.md](CHANNELS.md#its-language)). Its agents are told so by the component
`channel_language_prompt_provider`, which a personality wires in like the roster; one without it
leaves its agents to choose - their instructions, tools and the parts the runtime adds are English,
which pulls a model towards English. Nothing is translated: the conversation is one, in one
language, which every participant reads as it is.

The part reads the language through the turn's views (`ChannelView::language`) at the start of every
turn, so a change reaches the agent with its next turn. It names the language in English and in the
language itself, with its tag - "held in German - Deutsch, BCP 47 tag "de-CH". Write everything in
German ..." - since a model follows a name more surely than a tag; the names come from a small table
in the model (`Language::names`), found by the tag or what is left of it without its last subtags.
A language the table does not know is named by its tag alone. A channel held in no language adds
nothing. The part is public, and goes into the system prompt by default (`position`): the language
changes rarely. With `changes_only`, for an external agent, it is told once and then only when it
changed, taking back is told too ("no longer held in a particular language").

### What the session and channel are for

A session and a channel may say what they are for - a summary and a briefing (issue 245, see
[CHANNELS.md](CHANNELS.md#what-it-is-for)). Their agents are told by the component
`channel_briefing_prompt_provider`, which a personality wires in like the roster and the language;
where it comes in the personality's list decides where the part stands, and it belongs after the
personality's own instructions.

The part reads both through the turn's views (`SessionView`, `ChannelView`: `summary`, `briefing`)
at the start of every turn, so a change reaches the agent with its next turn - the session's first,
each under a heading of its own ("[About this channel]") that says it is what the place is for, as
its owner describes it: written by whoever may change the place, not by whom the agent talks with.
It steers what the agent says, never what it may do - permissions, approvals and the rules of a
place are untouched by it. A place that says nothing adds nothing. The part is public if it holds
only the session's descriptions, and classified as the channel if it holds the channel's. It goes
into the system prompt by default (`position`); with `changes_only`, for an external agent, it is
told once and then only when something changed, the whole of it again, taking back included.

### Its workspace

An agent is told of the workspace it may use in its channel (issue 251) by the component
`workspace_prompt_provider`, which a personality wires in next to its `terminal_tool`: its name,
classification and description, whether it has a shell - or that there is none here. The part reads
the workspace through the turn's views (`ChannelView::workspace`) at the start of every turn - its
files through the `WorkspaceManager` (`terminal_in`, issue 296) -, is classified as the workspace,
and goes into the system prompt by default (`position`).

It names the files at the workspace's root that say how to work on its files (issue 279): those
of the parameter `instruction_files` - by default `AGENTS.md` and `CLAUDE.md`, the convention of
the coding tools people use on the same files - that are there, each looked up with one
`Terminal::stat`. Every one found is named, since one often only points to the other, and the agent
is asked to read them with `read_file` before it works on the files. Their content does not go into
the prompt: the part changes only when one appears or goes, so the prompt cache holds, and what the
agent reads of them is knowledge of the workspace like any file it reads. One sentence adds that a
directory may hold such a file of its own, which applies there and takes precedence over one
further up; the component does not look for them. A storage that cannot be reached leaves the files
out, with a warning, not the turn.

### Instructions within a turn

The runtime steers the model within a turn, too: while a plan is executed, it tells the model which
step to work on next and, after the last, to write the final answer (`Turn::advance_plan`). Such an
instruction is a prompt part as well, placed with `Turn::place_turn_prompt` after the messages the
turn has produced so far - after the tool result that accepted the plan, say, never between a tool
call and its result. Like every part it is kept once by digest and never goes into the transcript:
the turn it was given in is told it again, later turns are not given it. The plan itself stays
visible to them through the `create_plan` call (issue 49).

An instruction is system text; where it ends up depends on the model, as described next.

### System messages

Some models' chat templates merge every system message into the system prompt and move it to the
front. A system message in the middle of the context then loses its place, and, since it changes the
system prompt, the prompt cache. Where a model keeps system messages is part of its profile
(`ModelProfile::system_messages`, issue 49):

| Rule | A system message after the first message that is none is given |
|---|---|
| `only_at_start` | as a user message - the default, which holds for every model |
| `anywhere` | as a system message, where it stands |

The rule is applied when the context is resolved, to prompt parts and to messages of the transcript
alike, and recorded with the profile; the system prompt at the front is a system message under
either rule.

### Control tokens in the content

Ollama tokenizes the content of a message with the model's special tokens enabled: `<|im_start|>`
typed by a participant, contained in a fetched web page or quoted by the model itself would become
the real token - a turn boundary the model cannot tell from one the runtime wrote, so that a message
could open a fake system turn (issue 149). Which strings are such tokens is a property of the
backend's tokenizer, not of the context, so the `ollama_client` defuses them, not the
`ContextResolver` (`llm/control_tokens.rs`):

*   **Which.** The model's tokens of type control (3) and user-defined (4) in `/api/show`
    (`verbose`); the latter are qwen3's `<think>` and `<tool_call>`, which Ollama tokenizes alike.
    Read once per Ollama server and model and shared by every client of the registry.
*   **How.** A zero-width space after the first character of every occurrence, searched for again
    until none is left. The model reads nearly the same characters, as text; nothing is removed.
*   **Where.** Every text the chat template renders, whatever the role: content, reasoning, tool
    calls and their arguments, tool results, and the names and descriptions of the tools offered -
    which an MCP server writes.
*   **If the tokens cannot be read,** the call fails with an `LLMError`, which ends the turn;
    nothing goes out unprotected. A failed read is not kept, the next turn tries again. A model that
    does not list its tokens cannot be used with the `ollama_client`.

The resolved context - what is recorded and shown as given - keeps the text as it was written; the
zero-width spaces exist only in the request.

### The turn's context and the resolver

What the model is given is not stored; it is resolved from what is (issues 49, 136,
`caelumora_model::context`):

*   **The turn's context.** A turn holds the `ContextRef` of its next message
    (`Work::context`): the digest of the transcript so far - its head - a `PromptRef` for every
    prompt part, the digest of the tools offered, the knowledge read and the model's profile. Which
    of the transcript's messages are the turn's own its messages say (`Message::turn_id`).
    References only - the turn keeps no copy of the transcript. `AgentChannelState::begin_turn`
    begins it on the transcript as it stands.
*   **The turn's messages.** What a turn says goes into the transcript at once, and into the
    history with the context it was produced from - the turn's as it stood - and the context moves
    on to the transcript's new head (`AgentChannelState::said`, issue 260). The turn keeps no
    message of its own, so nothing waits for its end, a turn given up included. Nothing is
    perceived during a turn - an agent takes in what others said between turns - and if it were,
    `perceive` moves the turn's context on as well, so that the context names the head either
    way.
*   **Placing the parts.** The `ContextProvider` places every part as a `PromptPart` - its text,
    its role and the transcript entry it follows - and the `AgentActor` sets them, with the
    knowledge they hold, with `Turn::set_context`. An instruction within the
    turn is placed after its last message (`Turn::place_turn_prompt`). Either way the text is kept
    in `AgentChannelState::prompts`, once per digest, and the reference goes into the turn's
    context, where it counts for the messages produced from then on - like knowledge read.
* **Resolving.** The `ContextResolver` resolves a `ContextRef` against the transcript, the prompt
    texts and the tool sets, for a turn the caller names - the turn under way, or the turn of the
    message recorded with the reference: the transcript it names, whose messages of that turn are
    its own, with every part after its entry, each as the profile gives it, and the tools offered.
    `Turn::full_context` resolves the turn's context and gives the model the result, a prompt part
    as a message in its role.
*   **Recording.** Each message is recorded with the context it was produced from as it is said,
    so what the agent saw can be asked for while the turn still runs. The same resolver resolves
    that `ContextRef` when it is.

What the model was given and what is recorded as given are the same reference, resolved by one
implementation, and therefore cannot diverge.

## Asking what an agent saw

What an agent saw is read through the views (`caelumora_runtime::view`, issue 163), from the member
of the channel that produced the message: the same message is in every participant's history, but
only one of them produced it.

*   `MemberView::context()` is the edge: it asks the `SecurityManager` with
    `ActionType::ReadAgentContext` and the agent's attributes (`Forbidden` otherwise; `NoContext`
    for a member that is no agent). Under the ownership policy only the agent's owner may, since a
    context holds more than the conversation: the system prompt, internal steps, tool results.
*   It reads the agent's state from the `StateStore`, where it is written after every step; the
    running loop holds the in-memory state. Every kind of member is a `MemberState`
    (`view::member`: a user, an agent, an approver, one that is gone). An agent's hands out an
    `AgentMember` with its state in the channel - the same for both kinds of agent (issue 206),
    so that its kind is not asked; its personality is read for its introduction alone, when
    that is first needed. An agent whose personality is gone keeps its state, read as any
    other. What the viewer may see of it, the member view decides for all of them alike, from
    the agent's document. The `AgentContextView` it answers with holds that state: the
    transcript, the private notes, and
*   `context_of(message_id)`, the context and the prompt parts the message was produced from
    (`ContextError::NotRecorded` if the agent did not produce it, `Unavailable` if what was recorded
    cannot be resolved any more). For an answer of an external agent it is not all its model was
    given, but the messages the remote agent was given in the one call that produced it (issue
    205) - also for a question back, the answer to it, and what a task says unasked, which has
    the context of the call that began the task - without prompt parts or tools, which are the
    remote agent's own. The whole conversation is in its history:
*   `history()`, all that was offered to the agent in the channel and what it said itself (issue
    207), as the messages stand now. It is one sequence for the agent as a whole and may differ
    from what the channel shows - an agent that joined late, was asleep or caught up after a
    restart heard what it heard. `has_context(message_id)` tells which of them `context_of` can
    answer for, `in_transcript(message_id)` which reached the transcript: not those the perceive
    processors dropped (issue 196); one they changed keeps its id and counts as passed on. The
    transcript itself is not shown on its own: what of it the model was given is the context of
    each message. The history is read as its items (`MessageHistory::items`, issue 258): the
    messages, and how each of the agent's turns ended - a turn given up with its reason, after what
    it said, whose messages' contexts are asked for like those of any other.

Of an external agent (issue 179) the view is the same, over the same `AgentChannelState` (issue
206): its transcript is what the remote agent was given; its notes, private notes and pending
approvals are empty, and its history holds no turn of its own, since it keeps none; and
`context_of` answers with what the remote agent was given in the call that produced the message
(issue
[205](../issues/205-external-agent-message-context.md)), as described above.

REST exposes it as `GET /sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context`. It
answers `403 context_forbidden`, `404 context_not_recorded` (not produced by that actor) or `404
context_unavailable` (recorded, but no longer resolvable). The response lists the knowledge read
with channel and agent names - a channel of another session is not named; one deleted is marked
`channel_deleted` to whoever could read it were it there (`Views::channel_of`, issue 164); files of
a workspace (`source` `"workspace"`) with its `workspace_id` - and the tools offered with the digest
of their set. Every prompt part and every knowledge read carries its
`classification` (`null` if not known), and every item and knowledge read `sets_label`: whether it
set the message's label (issue 170, `LabelCauses` in the REST server; the rule is in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

So that a client offers this only where it works, every member of a channel response carries
`can_read_context`, decided for the requesting user when the channel's snapshot is taken (see
[SECURITY.md](SECURITY.md#views)): for agents only, the same policy question `MemberView::context`
asks, whether the channel runs or not.

The web UI shows it in its context panel (issue 41), with the prompt parts where they were given,
the knowledge read and the tools offered; see [WEB-UI.md](WEB-UI.md#what-an-agent-saw).
