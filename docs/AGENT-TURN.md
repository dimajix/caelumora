# The Agent Turn

How an agent answers: the cycle its `AgentActor` runs, the `Turn` it works on, how a turn survives a
crash, and how it can be told afterwards what the model saw. What an agent remembers beyond a turn -
notes, private notes, memory - is described in [MEMORY.md](MEMORY.md); the components that take part
in a turn in [COMPONENTS.md](COMPONENTS.md); tools in
[TOOLS.md](TOOLS.md), approvals in [APPROVALS.md](APPROVALS.md).

## The cognitive cycle

`AgentActor::run` is a state machine over `ActorState`. Two states wait between turns, six belong
to a turn; `CarryingOutCommand` is entered and left while the agent waits (see [A command
turn](#a-command-turn)).

### Between turns

`Idle` and `Sleeping` wait for the next event, and take it in together with everything queued
behind it, in order, then commit once (`AgentBody::take_in`):

*   **Perceive.** A complete message of another participant is appended to the agent's history as
    it was said - all that was offered to it, live or caught up with on joining. What the agent
    *hears* of it are the sender's words (`Message::as_heard`): a `user` or `assistant` message with
    text, as a `user` message without its tool calls and results, which were the sender's; nothing
    of a tool message, an empty one or a `system` one. That is offered to the agent's
    `PerceiveInProcessor`s, which may drop, change or add messages; what passes is appended to the
    transcript. A failing stage is logged; what was heard stays in the history and is not offered
    again. Perceiving begins no turn and asks nothing of the policy. An external agent perceives by
    the same rule.
*   **Take note.** That someone joined or left the channel (`ParticipantJoined`, `ParticipantLeft`,
    issue 408) is recorded as the channel records it, into the history and into the transcript
    (`AgentChannelState::note_announced`), where it happened among what was heard - also what
    happened while the agent was not there, as it catches up. It begins no turn: the next one is
    told of it.
*   **Begin.** The last request to respond among what was taken in begins one turn; the earlier
    ones are dropped. The one turn answers the conversation as it stands, having heard every
    message, and acts in the contexts of the last request - with the authority of the last who
    asked, never of someone who did not. An agent that falls behind thus answers once when it is
    free again, not once per request that waited. A sleeping agent ignores the request; only
    `wakeup` ends the sleep. Whether it sleeps is read once everything is taken in, since the
    channel wakes an agent before it delivers the message that asks it.
*   **Answer again.** Of the requests to answer again in the batch, the one for the turn the agent
    gave up for it is accepted. Otherwise only the last is judged: if its message is the agent's
    last answer, that answer is taken back with the rest of its turn and a turn begins on the
    transcript as it stands; every other request is rejected (see
    [CHANNELS.md](CHANNELS.md#answering-again)).

### In a turn

*   **`Planning`** calls the model with the context and the agent's tools and streams the answer.
    With tool calls it registers them for approval where their tool requires it and goes to
    `ExecutingTools`; otherwise to `Reflecting`. A turn makes at most `MAX_MODEL_CALLS` model calls,
    counted across a restart, then it goes to `Reflecting`. Before a call whose context is too full
    by its `CompactionPolicy`, it goes to `Busy` instead (see [Compaction](#compaction)).
*   **`Busy`** does something of the agent's own accord, besides answering - the counterpart of
    `CarryingOutCommand`, which it does when asked. So far that is compacting its context; it
    tells the users what it does (`ActivityStarted`) and what came of it (`ActivityFinished`), and
    returns to `Planning` - stored before `ActivityFinished` is sent, so that whoever reacts to it
    reads the compacted context. A cancel ends it as it ends a call; a failure ends the turn.
*   **`ExecutingTools`** runs the calls that are ready. Once every call of the step has finished,
    the tool message goes into the turn and the agent returns to `Planning` - or to `Reflecting` if
    a tool asked for the turn to end. Calls that wait for a person take it to `WaitingForUser`. A
    cancel or stop interrupts the call under way, which is answered as interrupted, and starts no
    further one.
*   **`WaitingForUser`** waits for an approval decided or timed out, or questions answered or
    declined, and returns to `ExecutingTools`. Other events stay queued until the turn is over.
*   **`Reflecting`** takes the turn over, stores the state and announces its end. The agent then
    lands in the state a tool requested (`Sleeping` after `stop` or `bye`) or in `Idle`.
*   **`Cancelling`** takes the turn over like `Reflecting` and returns to `Idle`. Calls of the step
    still without a result are answered as not executed, so that the transcript never holds a call
    without an answer.

An error in a turn ends the turn, not the agent: it is logged, the turn is given up as `Failed`
and the agent returns to `Idle` (see [CODING-GUIDELINES.md](CODING-GUIDELINES.md)).

### What leaves the agent

Every message passes the agent's `EmitOutProcessor`s and is published through the turn, which leaves
out what only the agent's owner may see (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)). A complete
message is *said* (`Turn::say`): kept in the turn, then published, so that the channel learns of
every message the turn keeps as it is made. A partial one is only *shown* (`Turn::show`). Both take
the actor's output as a parameter: the turn is handed to every component working in it, and none of
them is to speak in the agent's name.

While a plan runs, the messages of its steps are *internal*: kept like every message and published
as intermediate, but heard by no other participant and not the answer. The answer after the last
step is. The instructions for the steps are prompt parts, not messages (see [Instructions within a
turn](#instructions-within-a-turn)).

The agent announces its state to the users as it changes (`StateChanged`), each state once, in its
own contexts rather than a turn's: its state is the agent's, not part of an answer. One move is not
the run loop's: the channel wakes a sleeping agent itself, synchronously, because the moderator,
deciding right after whom to ask, leaves the sleeping out; the run loop is notified and announces
`Idle`.

## The `Turn`

The agent's state in its channel is its `AgentChannelState` - history, transcript, notes, the texts
of its prompt parts; its state across channels is its `AgentState`. A `Turn` is the handle to both
while a turn is under way, and only then: between turns the actor works on the state itself, and
taking a turn over or giving it up uses the handle up.

What a turn does to the state - beginning, saying something, finishing, giving up - is a rule of the
state, kept with it in the model (`AgentChannelState::begin_turn`, `offer_tools`, `said`,
`finish_turn`, `give_up_turn`). `Turn` calls these and does what runs around them.

The working state of a turn is one value, `AgentChannelState::turn`, `None` between turns. Its
`Work` is what every piece of work of an agent has, a turn as much as an external agent's exchange
with its remote agent (`caelumora_model::work`):

*   which turn it is (`TurnId`),
*   the envelope it answers and the authority it acts with,
*   the classification of its channel as the turn began, which bounds what it may read,
*   its label - the join of everything it has seen, at most the channel's - which everything it
    writes is classified as (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-a-turn-writes-as)),
*   the context its next message is produced from (see [The turn's context and the
    resolver](#the-turns-context-and-the-resolver)).

The rest is the turn's own: the changes staged to the agent's states, the tool calls, the plan, the
state a tool requested.

**The turn's situation** (`Turn::situation`, issue 399) is what is read of it: where the piece of
work stands and what it sees from there - who works, in which channel and session, in whose name,
bound by which classification, what it has seen so far (its label), the agent's states, the tools
it was offered, and its session to navigate. A `Situation` is what every piece of work of an agent
has, so an external agent's work has one too. The `Turn` reads through it and passes none of it
on: its own methods change something - stage, say, give, compact -, act in the channel, or read
what only the turn holds (its id, what it staged, the state a tool requested).

**The turn's id** is given when it begins and stored with it, so that a turn resumed after a restart
is the same turn. Every message the turn produces carries it (`Message::turn_id`), so which messages
belong to one turn can be told from the messages alone. It is not covered by the digest, and a
modification does not take it over: a changed message stays in the turn it was produced in.

Beside its state, a turn reaches its session through its views (`Situation::session`), from which
components read other channels as the confidentiality rules allow - at the classification the turn
began with (see [MEMORY.md](MEMORY.md#reading-other-channels)). The views only read: its session,
its channel and the workspace it may use there (`Situation::session`, `channel`, `workspace`) are
views, named after what they show, and answer alike - nothing where the work reaches no session,
an error where it may not read what it reaches. What is done with the workspace's files goes
through a handle instead, named after the handle: its terminal, to read them
(`Situation::workspace_terminal`), and to change them or run commands, the turn's
(`Turn::workspace_terminal`, `Turn::workspace_shell`). Both are reached through the actor's
`ActorRuntime`, which the situation and the turn hold and which their components never see: they
are given their methods (issue 370).

### Beginning and ending a turn

*   **Beginning** fixes the turn's id, envelope, authority and classification. The classification
    and the publication hold for the whole turn, so that what it reads, publishes and remembers is
    judged alike even if the channel's level changes meanwhile.
*   **Taking a turn over** (`Turn::commit`) records its end, `TurnFinished`, after its messages -
    which are in the transcript and history since they were said - applies the changes it staged
    and clears the working state.
*   **Giving a turn up** (`Turn::give_up`) - failed, left, asked for again - unsays nothing: its
    messages stay where they were said, and its end follows, `TurnGivenUp` with the reason. Nothing
    it staged is applied. What its tools did stays done; calls still without a result are answered
    as not executed. The model is told why the turn ended by a fixed text that says nothing of the
    conversation.

A turn that no longer flows into its channel - the channel was lowered below it - is given up as
`Lowered`, whatever ended it, and of it the model is given only which tools it called and how they
ended, classified as the channel is now: what it read and said after the lowering was never
released. Before every step the agent asks whether the turn still flows into its channel
(`may_publish` of the turn's classification, in the agent's name); once it does not, the turn is
given up as `Lowered` and the users are told (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

`cancel` takes a turn over rather than giving it up, unless it was lowered. A turn whose answer is
asked for again while under way is given up as `Retried` after the step it cut short, and the
request on its way begins the next turn.

Either way the turn hands back the contexts it had as an `Answering`, which sends its last event
with the publication the turn had and withdraws the approval requests of the calls it abandoned. An
external agent publishes its answers through the same `Answering`.

**A message given** to the agent to read - a skill a member or the model loaded, the summary of a
compaction - is recorded by `Turn::give`: into the transcript as a `Given` entry, which keeps what
knowledge it is, if it is knowledge - of an entity, `KnowledgeSource::Entity { kind, id, digest }`
(issue 323) - and none for a summary (issue 389); into the history without a context, since the
agent did not produce it. It is not published, the turn has seen it, and its digest is that of the
message, since the model is given the same. A tool gives what it gives while it runs, but nothing
may stand between the calls of a step and their results, which the models refuse: while the calls
run, what is given is put aside with the turn (`TurnState::given_after_step`) and written right
after the step's tool message.

### A command turn

A command turn carries out a command a user asked the agent for, rather than answering
(`AgentChannelState::begin_command_turn`): no tools, the command kept with the turn, and the
component that carries it out stages on it as a tool does. Taken over, it records the command
carried out and by whom it was asked; not carried out, it is dropped without a trace. It announces
no answer, is not the turn a cancel or a retry reaches, and is never stored under way. Admitted, it
tells the issuer alone that it began. An awake agent stands at `CarryingOutCommand` meanwhile, which
no cancel ends; a sleeping one carries the command out asleep and stays `Sleeping`. See
[COMMANDS.md](COMMANDS.md#commands-of-an-agent).

### Storing and resuming

The state is stored after every step, together with the state that comes next: what a crash leaves
in the store is the step still to do. What a step writes is what changed (see
[STORAGE.md](STORAGE.md#what-a-state-tells-a-store)). When the agent is loaded again, the turn is
resumed:

*   `Planning` asks the model again with the turn so far.
*   `Reflecting` takes the finished turn over.
*   `WaitingForUser` keeps waiting.
*   `Cancelling` ends the turn.
*   `ExecutingTools` is the exception. A tool may have run without its result being stored, and
    running it again would repeat its effect. So every call that was ready to run is answered as
    *not executed*, and the model decides again.

A turn state stored without the turn's working state cannot be resumed; the agent starts `Idle`.

An agent ends its task in one of two ways (see [CHANNELS.md](CHANNELS.md#ending-an-actor)):

*   **Stopped** - its session is stopped, or the process shuts down - it keeps its turn: the step
    under way is interrupted and the turn stored at the step it stood at, to be resumed as above
    when the session runs again (issue 377). A model call is made again, and a tool call that ran
    is answered as interrupted because the session was stopped; a call that had not run yet is
    answered as not executed, as after a crash. An approval it waits for stays pending and is asked
    for again. Nothing is given up and nothing announced.
*   **Leaving** - removed from its channel, or the channel or its session deleted - it gives up a
    turn under way (`GivenUpReason::Left`), says so, stores itself idle and ends its task.

Either way the channel aborts the task only if it does not end within `STOP_GRACE`.

## What an agent saw

For every message an agent produced, it can be told what its model saw when it wrote it.

### The transcript as a chain

The `Transcript` is what the agent's context is built from: its own messages in full, with tool
calls and internal steps, and what others said as it received them. The `History` is all that
was offered to the agent and what it said, with how its turns ended. The history is informational:
it is the record of what was said and what it was produced from, for people to read, and it is never
read to build what the model is given - that is built from the transcript alone (issue 387). Both
are a `caelumora_model::chain::Chain` of entries, each with a `Digest` over its content and the
digest before it, so one digest names everything up to that point by content. What a message's
digest covers is what the model sees of it, not what only describes it afterwards
(`digest::message_chain`). A message does not name its predecessor: the same message is in several
chains with a different one in each, so the predecessor belongs to the chain.

Each chain holds one entry type, an enum of what can happen; what stands is read from the entries.
Notes and memory are chains too, ledgers of numbered items (see [MEMORY.md](MEMORY.md)).

A chain is a tree with a head:

*   Appending makes the new entry the head; moving the head back leaves what followed as a branch
    no longer active. Nothing is removed, and nothing changes an entry once added.
*   What the chain says is its *active path*, root to head: the conversation as it stands. What
    must consider every branch - looking up a message by id, the next number of a note - reads the
    entries of every branch, and only as far back as it has to.
*   A digest recorded earlier finds its entry on any branch, so a reference into a branch no longer
    active still resolves.
*   An entry appended with the digest of one already there is that entry: the head moves to it.
*   Every entry has a *position*, the order it was added in across every branch, beside its
    digest: the digest names what the chain held up to it, the position where it is. A range of
    the chain - the entries after a position, a page before one - is read by positions.
*   An external agent's transcript starts anew with every call, since the remote agent keeps what
    it was given: every call before is a branch from the root.
*   Whoever holds a chain in memory may hold only a window of it, from a position on; the store
    holds all of it. A digest whose entry lies before the window is found in the store, and an
    entry appended with the digest of one only the store holds is added a second time - the same
    content after the same predecessors, which the store keeps both of (see
    [What is held in memory](#what-is-held-in-memory)).

Neither ending a turn nor changing a message moves a head: each is an entry after what it is about.
Nor does the end of what a compaction wrote, which is an entry of the transcript after it (see
[Compaction](#compaction)).

A digest is stored with its entry and never recomputed: it records what was sent to the model. Were
the scheme to change, old entries keep their digests and only new ones get the new scheme's.

### Changes to a message

A message once said can be modified or taken back by its author, and reclassified. None of these
rewrites it: the change is an entry of its own after it, in every chain that holds the message. A
chain that does not hold it, or where the message as it stands cannot be changed so, records
nothing. What stands is read by one rule, kept with the message (`Message::modify`, `revoke`,
`reclassify`):

*   a modified message keeps its place, id and time, and takes over content, reasoning,
    classification and mentions. Its tool calls and results stay, so that a call and its result
    cannot come apart;
*   a message taken back keeps its place and loses what it said. The history keeps it so, for
    whatever names it by id; the model is no longer given it;
*   a reclassified message takes over the classification alone; one taken back is not reclassified.
    A turn begun afterwards has seen the message as it stands, so a message released no longer
    raises the label of what follows.

In the transcript, a change applies only within the path that holds it, so a reference taken
before the change resolves to the message as the model saw it then, and one taken after to what
stands. In the history, a change applies to the message on whichever branch, as a change made in the
channel does. Either way a change comes after what it changes, so the messages of a range of the
chain that runs to its end are read as they stand from that range alone: what asks for one message,
or for what a turn said, reads from where the message was said or the turn began, not from the start
(`HistoryEntry::fold_into` is the history's rule, `history::fold` reads a range by it).

An external agent's remote agent was given the message on a branch where the change is not recorded,
so the external agent tells it instead - the message as it was given read from the store, since that
branch lies before the one the actor holds: it appends a *notice* to its transcript, in the name and
role of the message's sender, given with the next call. The remote agent is told once, of the
message as it stands; a reclassification is not told, since it knows no labels. A notice only tells:
nothing in its wording asks the remote agent to disregard something, which a filter of the remote
side may take for an injection. An agent run here tells nobody.

### What is held in memory

An agent's actor holds its history only from its window on: the last 200 entries, and during a turn
everything from the turn's first message on; the store holds all of it (see
[STORAGE.md](STORAGE.md#histories), issues 379, 387). The window is the same for both kinds of agent
and does not follow the transcript or its compactions: nothing that builds what the model is given
reads the history. What reads it in memory is informational, reads from the end, and lies in the
window:

*   what a turn said (`History::said_in_turn`), its answer, read from where the turn began;
*   which of the messages a request to answer again takes back were said, to announce them;
*   its last message, whether its last turn was given up, and the digest an entry pushed again is
    found by.

A change of a message before the window is recorded all the same, and the store applies it to the
message. What reads further back - what the agent saw when it produced an old message, a page of its
history - reads the store, through the views. Catching up when it joins looks for the last message
it holds that the channel's history has, within the window; finding none, it takes in what it may
see from where its view begins.

Of its transcript the actor holds the active branch alone, from its root on (`transcript_from`,
issue 380): right after the transcript began anew - a compaction, an external agent's call -,
nothing. Every reader in memory reads that branch: a turn and its next call, a compaction, how full
the context is, answering again, taking in a change of a message. What lies on an older branch is
read from the store (`TranscriptStore`): the path to a digest, for the context of an older message;
which messages reached the transcript, for a page of the agent's history; and, for an external
agent, a message as its remote agent was given it, before it tells of a change (`tell`, through
`ActorRuntime::transcript_message`). Its sums are parts of its state, not read over the branches:
the tokens used (`used`) and the classifications the remote agent was given (`RemoteState`).

Of the prompt texts and the tool sets, kept once by digest, the actor holds those its turn under way
names, and between turns none (issue 383): the turn's calls, how full its context is and the tools
its calls are checked against read them, and nothing reads them between turns - how full the context
is then is kept, not read (issue 390), answering again begins a turn that assembles its parts anew,
and an external agent tells what it gave by digest. A text kept again in the next turn is written as
the same. What the context of an older message names is read from the store, with the path of its
transcript (see [STORAGE.md](STORAGE.md#prompt-texts-and-tool-sets)).

### What is recorded

Every message a turn says is recorded in the history with the `ContextRef` it was produced from: the
digest of the transcript before it, the prompt parts, the digest of the tools offered, the knowledge
read (see [MEMORY.md](MEMORY.md)) and the model's profile. The turn's own messages are those
carrying its id. The `ContextResolver` resolves such a reference into what the model saw, in the
order it was given it.

### Tools offered

A turn offers its model the tools the agent's providers offer for its work as it begins: once it is
begun, and before its context is built, so that a provider can ask the turn's work - its place and
whose authority it acts with - which of its tools may be called there
(`AgentChannelState::offer_tools`, see [TOOLS.md](TOOLS.md#tool-sets)). They are fixed in the turn
like its classification - a resumed turn keeps them. The model is told of a tool only its name,
description and parameters; whether its calls need approval, who sees them and how results are
classified stay with the runtime. A tool that changes during a turn is offered as it is from the
next turn on; a call of one that is gone fails as unknown. The tool sets offered are kept once per
digest, each tool whole with how its calls were treated: the same tools treated otherwise are
another offer. The model gets the tools from the resolved context, as it gets the messages, so what
it was offered and what is recorded as offered are the same.

### Reasoning given back

Which reasoning of earlier messages a model is given again is a property of the model, declared in
its `ModelProfile` as a `ReasoningHistory`:

| Rule | The model is given |
|---|---|
| `never` | no reasoning at all |
| `current_turn` | the agent's own reasoning of the turn under way - the default |
| `always` | the agent's own reasoning of every turn |

A model is never given another actor's reasoning. The profile is fixed when a turn begins and
applied in one place, the `ContextResolver`, so a backend gets only what its model may see. The
stored transcript keeps all reasoning, and the digests name it, not a filtered copy.

### Who said what

What others say reaches an agent as `user` messages, and a model's input has no place for their
sender. So the sender is marked in the content, as the profile says (`sender_format`,
`sender_style`; which suits which model is in [LLM-MODELS.md](LLM-MODELS.md)):

| `sender_format` | A message of another participant is given as |
|---|---|
| `none` | its content alone |
| `prefix` - the default | `@alice: text` |
| `xml` | `<message from="@alice">text</message>` |

`sender_style` names the sender by `handle` (the default) or `name`, taken from the message, so that
a stored transcript reads the same after a renaming. Only what a user or another agent said is
marked, never the agent's own messages: a model whose own messages carry the marker imitates it and
continues the conversation as a transcript. The marking is applied by the `ContextResolver`, so the
stored transcript keeps the message as received. How messages are marked is explained to the model
with the roster ([who is in the channel](#who-is-in-the-channel)); without that, models imitate the
marker. An external agent is given what was said by the same rule.

A marker is a claim like any text: a participant can write another's into their own message, and
models believe it. Nothing with consequences may rest on whom the model believes said something.

### Tokens used

Every message the model produces carries the tokens of its call and the size of the context the call
had. What an agent used in a channel is a running total kept with its state, counted as each of its
calls' messages is recorded - said in a turn, or written as a summary in a compaction - so that a
call is counted once, on whichever branch, the turn under way's included; it is not summed over the
transcript, whose older branches need not be at hand (`AgentChannelState::used`). Two groups mean
different things:

*   **context** - how full the context is against its size (`TokenUsage::context_tokens`): the
    last call's prompt and answer, as counted. A compaction replaces it once, when it is made:
    the summary's call measured the old branch, so the new one is estimated then, by the
    `ContextResolver`, with what the turn's model is given next (issue 390). It is kept, never
    read anew however often it is asked for; what was added since the last call - a tool's
    result, what was heard - is not in it until the next call measures it. How full the context
    is before a call, the compaction reads for itself (`ContextResolver::token_positions`): from
    the transcript alone, at a call's `prompt_tokens` before a message of the agent's model added
    where it stands - after where the compaction that began the branch ends, if one did
    (`TranscriptEntry::Compacted`) - and elsewhere estimated from what the model is given, as it
    is given it (`ContextItem::estimated_tokens`);
*   **usage** - the total of all calls: what was processed. Every call is given the whole context
    again, so the totals grow much faster than the context and are no measure of how full it is.

An external agent's calls report no tokens, so its usage is unknown, not zero.

### Compaction

All of this relies on nothing ever being removed from the transcript. A compaction
(`AgentChannelState::compact`, through `Turn::compact`) produces a new context that came from the
old one but does not continue it:

*   **A branch of its own.** The transcript begins anew from the root with what replaces the older
    messages - a summary, what is given again, each a `Given` entry, the summary without a source -
    and then copies of the entries kept, as they stand. Every copy gets a digest of its own, since
    what precedes it is another.
*   **The old branch stays**, in the store. Every reference recorded before resolves to what it
    did; the resolver does not know there was a compaction.
*   **Prompt parts are carried over.** A part the turn placed after an entry kept follows its
    copy, one placed after an entry replaced follows the replacement - the notes before the
    turn's input as well as the step of a plan. A plan whose instructions were replaced is named
    again where its current step stood: its steps, which are done and which is under way, then the
    instruction (`Turn::compact`).
*   **The turn's answer is what it said**, read from the history (`History::said_in_turn`),
    not from the transcript, of which a part may have been summarised.
*   **Where its part ends** is an entry of the transcript, `TranscriptEntry::Compacted`, after the
    replacement and the copies: what comes before it on the branch was written by the compaction
    and carries the measures of calls made on another branch, what comes after was added where it
    stands. It gives the model nothing, and its digest covers its place alone. An earlier
    compaction's end is not copied, and an answer followed by one is not answered again: it may be
    in a summary already.
*   **Where it came from** - the old branch, the turn - is recorded in the history
    (`HistoryEntry::Compacted`).
*   **Counted once.** The tokens used count a message kept where it was first recorded.

The mechanism takes the replacement and the first entry kept, and whose the replacement is;
components decide the rest. When the agent compacts on its own is its one `CompactionPolicy`'s
(issue 284), how its one `CompactionStrategy`'s - asked by two triggers with the same code: the
agent itself before a model call, and `/compact`, which the `compaction` component carries out.
Before every call of a turn - the first of the next turn as well, with what was heard since - the
actor asks the policy whether the context is to be compacted (`CompactionPolicy::is_due`), at most
once before each call. The default, `threshold_compaction_policy`, says so once the context fills
`threshold` of the model's context size, as soon as a call made on the active branch measured it;
a model that reports no context size gives nothing to measure against, which it says once. The
agent then compacts in the state `Busy`, telling the users as an activity
(`ActivityStarted`, `ActivityFinished`); the summary's call does not count against
`MAX_MODEL_CALLS`. A compaction that fails gives the turn up: the call it was to make room for
would not fit. `/compact` tells of itself as a command does.

The default strategy, `summary_compaction`, keeps `keep` of the model's context size as it stands.
It cuts only before a message, by the positions the resolver measured (`token_positions`): within
the turn under way before a call of its model but the first - before that the turn's input would
go and the turn stay -, before the turn before any message but a tool's result, up to the turn's
input; never into a turn given up as `Lowered`. So a tool's result stays with its call, and the
newest call with its results is never summarised. The agent's own model summarises the rest. A
summary that holds messages of the turn under way is the turn's (`Message::turn_id`) and classified
as the turn's label - it holds what the turn read, and goes with it if the turn is given up as
`Lowered`; one of what precedes the turn alone is no turn's and classified by what it summarises,
since its call was given the transcript alone.

What the agent had been given in the part summarised and is not still there in the part kept is
given again after the summary, as it is now, by the component that gave it: a `KnowledgeRestorer`
(issue 323), a facet that names its kind and answers, of the entities of that kind given - each
a `Given` entry of the transcript whose source is a `KnowledgeSource::Entity` (issue 389) -, which
it gives again and as what text. What was given without a source - an earlier summary - is
summarised with the rest. The `skills` component gives the skills the agent may still load there
([SKILLS.md](SKILLS.md)); what no component gives again - a workspace's files - is not, and the
strategy knows no kind itself. `/compact` tells the channel what was given again, by kind and name,
and how much could no longer be given.

## Prompt parts

What the model is given besides the conversation - the system prompt, notes, what the agent
remembers - is a *prompt part*. The `ContextProvider` decides where the parts go; the turn records
what it was handed. A part is never stored in the transcript, which stays a stable prefix.

Every part the runtime writes has one form: a short title in brackets on a line of its own -
`[Who is in this channel]`, `[Your private notes]` - then what needs explaining, then the content
(`prompt::titled`). A part that says nothing is no part. The personality's own system prompt stays
as written.

### The `ContextProvider`

The `ContextProvider` only answers: given the turn's `Situation`, it returns the prompt
parts, each where it goes, and the knowledge they hold, and the actor sets both at once
(`Turn::set_context`). It runs once, at the start of a turn, so a part stays in place across the
tool loop. When it fails, the turn fails, whatever its fail policy: without a context there is
nothing to give the model.

### Parts and their position

A `PromptProvider` contributes a part at a `PromptPosition`:

*   **`Start`** - the system prompt, before everything, as a system message.
*   **`BeforeTurnInput`** - transiently before the newest message of the turn, as a user message.

The distinction exists because many models cache the prompt up to the first change, and some merge
every system message into the system prompt. Information that changes often does not belong in the
system prompt.

The parts of one position are joined in the order of the configuration. Every part is classified as
its provider says, or as the channel; the turn has seen it. A part whose component fails open is
left out of that turn with a warning; one that fails closed fails the turn.

A `PromptProvider` is given a `Situation` (`Turn::situation`), what every piece of work of an agent
has, so the same providers serve an external agent as well. What only reads takes a `Situation`;
what stages changes takes the `Turn`.

The runtime brings four parts of its own, which a personality wires in as components. They go into
the system prompt by default, since they change rarely. For an external agent, whose remote agent
keeps its conversation, the first three take `changes_only`: told once, then only when they changed.

### Who is in the channel

`channel_members_prompt_provider` lists the agent itself and every other current user or agent,
with their introductions, read at the start of every turn. A person's introduction is marked as
written by them, so that it does not read as an instruction of the system, and kept to one line. It
also says that `@all` addresses all of them, and explains how the others' messages are marked.

That the roster changed is told where it happened: a transcript entry of someone who joined or left
is given to the model as the channel's notice - "Channel notice: @alice added @bob to the channel.",
"... joined", "... left", "... was removed", the agent itself as "you" - named in the `sender_style`
of its profile and unmarked by any participant, as a system message where the model keeps one
(`SystemMessages`), else as a user message. The ContextResolver words it, so that the transcript
keeps what happened rather than words; an external agent's remote agent is given the same
(`ModelProfile::given_messages`).

### The channel's language

`channel_language_prompt_provider` tells the agent the language its channel is held in (see
[CHANNELS.md](CHANNELS.md#its-language)), by name rather than by tag alone, since a model follows a
name more surely. Nothing is translated.

### What the session and channel are for

`channel_briefing_prompt_provider` gives the summary and briefing of the session and the channel
(see [CHANNELS.md](CHANNELS.md#what-it-is-for)). It steers what the agent says, never what it may
do: permissions and approvals are untouched by it.

### Its workspace

`workspace_prompt_provider` describes the workspace the agent may use in its channel and names the
instruction files at its root (`AGENTS.md`, `CLAUDE.md`) for the agent to read - by name only, so
that the prompt does not change with their content.

### Instructions within a turn

While a plan is executed, the runtime tells the model which step to work on next and, after the
last, to write the final answer (`Turn::advance_plan`). Such an instruction is a prompt part placed
after the messages the turn has produced so far - never between a tool call and its result. It is
given again within its turn, never to later turns, which see the plan through the `create_plan`
call.

### System messages

Some models' chat templates move every system message to the front, so that one in the middle loses
its place and the prompt cache. Where a model keeps them is part of its profile
(`ModelProfile::system_messages`):

| Rule | A system message after the first message that is none is given |
|---|---|
| `only_at_start` | as a user message - the default, which holds for every model |
| `anywhere` | as a system message, where it stands |

### The turn's context and the resolver

What the model is given is not stored; it is resolved from what is (`caelumora_model::context`):

*   **The turn's context** is the `ContextRef` of its next message: references only, the turn keeps
    no copy of the transcript.
*   **What a turn says** goes into the transcript at once, and into the history with the context it
    was produced from; the context moves on to the transcript's new head
    (`AgentChannelState::said`). Nothing waits for the turn's end, a turn given up included.
*   **Prompt parts** are kept once per digest; their references go into the turn's context, where
    they count for the messages produced from then on.
*   **Resolving** - the `ContextResolver` resolves a reference against the transcript, the prompt
    texts and the tool sets, each as the profile gives it. The turn's next call and every later
    question about a message go through it: the turn against the active branch, the texts and the
    tool sets the agent's state holds (`ContextResolver::of`), a question about an older message
    against the path, the texts and the tool sets its reference names, loaded from the store
    (`ContextResolver::on_path`).

What the model was given and what is recorded as given are the same reference, resolved by one
implementation, and therefore cannot diverge.

## Asking what an agent saw

What an agent saw is read through the views (`caelumora_runtime::view`), from the member of the
channel that produced the message: the same message is in every participant's history, but only one
of them produced it.

*   `MemberView::context()` asks `ReadAgentContext`. Under the ownership policy only the agent's
    owner may, since a context holds more than the conversation: the system prompt, internal steps,
    tool results. It reads the agent's stored state, the same for both kinds of agent.
*   `context_of(message_id)` resolves what the message was produced from, as the message's item in
    the agent's history records it, read from the store, against the path of the transcript it
    names, loaded from the store on whichever branch it lies. For an external agent it is what its
    remote agent was given in the call that produced it, without prompt parts or tools, which are
    the remote agent's own.
*   `history_page(before, limit)` is a page of all that was offered to the agent and what it said,
    as the messages stand now - which may differ from what the channel shows, for an agent that
    joined late or was asleep -, read from the store. `in_transcript` tells which of a page's
    messages reached the transcript, on whichever branch - not those the perceive processors
    dropped -, asked of the store once for the page.

REST exposes it as `GET …/actors/{aid}/messages/{mid}/context` (see
[REST-SERVER.md](REST-SERVER.md)), the web UI in its context panel (see
[WEB-UI.md](WEB-UI.md#what-an-agent-saw)). So that a client offers it only where it works, every
member of a channel response carries `can_read_context`, decided by the same policy question when
the snapshot is taken.
