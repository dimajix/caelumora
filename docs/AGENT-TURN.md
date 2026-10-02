# The Agent Turn

How an agent answers: the cycle its `AgentActor` runs, the `Turn` it works on, how a turn survives a
crash, and how it can be told afterwards what the model saw. What an agent remembers beyond a turn -
notes, private notes, memory - is described in [MEMORY.md](MEMORY.md); the components that take part
in a turn in [COMPONENTS.md](COMPONENTS.md); tools and approvals in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

## The cognitive cycle

`AgentActor::run` is a state machine over `ActorState`. Two states wait between turns, five belong
to a turn.

**Between turns** (`Idle`, `Sleeping`) the agent waits for the next event:

*   **Perceive.** A `FullResponseGenerated` of another participant is appended to the agent's
    history - all that was offered to it - and passed through its `PerceiveInProcessor`s, which may
    drop, change or add messages; what passes is what the agent processes, and is appended to its
    transcript - whatever the personality configures (issue 196). No turn begins, and nothing is
    asked of the policy (issues 85, 90).
*   **Begin.** A `ResponseRequested` from the moderator begins a turn (`Turn::next_turn`, see
    below), announces `ResponseStarted` and has the agent's `ContextProvider` assemble the context.
    The agent goes on in `Planning`. A sleeping agent ignores it; only `wakeup` ends the sleep.

**In a turn** the agent moves through these states:

*   **`Planning`** calls the model with the context and the agent's tools and streams the answer.
    Partial messages are published as they arrive. When the complete message contains tool calls,
    the agent registers them for approval where their tool requires it and goes to `ExecutingTools`;
    otherwise to `Reflecting`. A turn makes at most 25 model calls (`MAX_MODEL_CALLS`), then it goes
    to `Reflecting`. The count (`TurnState::model_calls`, `Turn::count_model_call`) is stored with
    the turn, so a turn resumed after a restart goes on counting.
*   **`ExecutingTools`** runs the approved calls that have no result yet. Once every call of the
    step has a result, the tool message goes into the turn and the agent returns to `Planning` - or
    to `Reflecting` if a tool asked for the turn to end, as the stop tool does. If calls are still
    pending, it goes to `WaitingForApproval`.
*   **`WaitingForApproval`** waits for `ApprovalGranted`, `ApprovalDenied` or `ApprovalTimedOut`,
    records the decision and returns to `ExecutingTools`. Other events stay queued until the turn is
    over. See [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md) for the approval process.
*   **`Reflecting`** takes the turn over (`Turn::commit_turn`), stores the state and announces the
    end with `FullResponseGenerated`. The agent then lands in the state a tool requested (`Sleeping`
    after `stop` or `bye`) or in `Idle`.
*   **`Cancelling`** is entered when the channel cancels the turn. It takes the turn over like
    `Reflecting` - so observers learn that generation stopped - and returns to `Idle`. Calls of the
    step still without a result - waiting for an approval, or approved but not yet run - are
    answered as not executed because the turn was cancelled, so that the transcript holds no call
    without an answer (issue 148).

Every message that leaves the agent passes its `EmitOutProcessor`s first, of which there is no
implementation yet (issue 99), and is published through `Turn::send_event`, which leaves out what
only the agent's owner may see (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

When a plan is in progress (`create_plan`), the messages of its steps are *internal*: they are
published as intermediate messages, kept in the transcript, but not in the history. The answer after
the last step is the final one. The instruction for each step, and the one to write the final
answer, are prompt parts placed among the turn's messages, not messages (see [Instructions within a
turn](#instructions-within-a-turn)).

An error in one of the turn's states ends the turn, not the agent: the agent logs it, gives the turn
up (`Turn::give_up_turn`), emits `TurnFailed` and returns to `Idle` (see
[CODING-GUIDELINES.md](CODING-GUIDELINES.md)).

## The `Turn`

The agent's state in its channel is its `AgentChannelState`: its `MessageHistory`, its `Transcript`,
its notes and the texts of its prompt parts. Its own state across channels is its `AgentState`. A
step of the cycle works on a `Turn` (`caelumora_runtime::agent_turn`), the handle to both.

The working state of a turn is one value, `AgentChannelState::turn: Option<TurnState>`, which is
`None` between turns (issue 85). It holds what the turn is and what it has done so far:

*   the envelope it answers (`EventContext`),
*   whose authority it acts with (`SecurityContext`),
*   the classification of its channel as the turn began - where the channel is, the turn reads from
    the envelope it answers - which bounds what it may read,
*   its label: the join of the classifications of everything it has seen so far, at most the
    channel's, which everything it writes is classified as (issue 168, see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-a-turn-writes-as)),
*   what its next message is produced from (see [The turn's context and the
    resolver](#the-turns-context-and-the-resolver)), the messages produced, each with what it was
    produced from,
*   the changes staged to the agent's states,
*   the tool calls, the plan and the state a tool requested.

Work between turns does not touch it: perceiving a message only appends to the transcript and the
history.

Beside its state, a turn reaches the agent's session: `Turn::session()` answers the `SessionView` of
the turn's views (issue 163), from which the components read the other channels as the
confidentiality rules allow - at the classification the turn began with (see
[MEMORY.md](MEMORY.md#reading-other-channels)). A turn outside of a channel, in tests, has none and
reads nothing.

### Beginning and ending a turn

*   **`Turn::next_turn`** begins a turn and fixes the first four of the values above. The
    classification and the publication are taken once and hold for the whole turn, so that what the
    turn reads, publishes and remembers is judged alike, even if the channel's level changes
    meanwhile. All four are stored with the channel state, so that a turn resumed after a restart is
    the same turn.
*   **`Turn::commit_turn`** takes a completed turn over:
    *   it appends the agent's messages to its transcript, and the non-internal ones to its history,
        each with a `ContextRef` (see below);
    *   it applies the changes staged to the agent's state in the channel, and those staged to its
        own state;
    *   it clears the working state, so that none of it is stored with the channel state.

    It returns a `CommittedTurn`: the final messages for `FullResponseGenerated`, whether the
    channel state changed, and the ended turn.
*   **`Turn::give_up_turn`** gives up a turn that failed or was stopped (issue 145). No end of a
    turn loses its messages:
    *   its messages are appended to the transcript and the history as by `commit_turn`, each with
        its `ContextRef`, and both heads go back to where the turn began: the messages stay as a
        branch no longer active (see [The transcript as a chain](#the-transcript-as-a-chain)). So
        the agent's model does not see them, nor does the channel, while what they were produced
        from can still be asked for;
    *   the transcript's last message of the turn is marked `given_up`, with the reason (failed,
        with what its users were told; lowered, with the channel's new classification; stopped), the
        time and how many messages were the turn's;
    *   nothing it staged is applied - no note, nothing remembered;
    *   what its tools did stays done. If a call ran (its result has a start), the agent is told on
        the active branch, in an internal system message of the runtime in its transcript - with
        arguments and results, classified as the turn's label, while the turn may still flow into
        the channel (`ActionType::Publish`); only the names of the tools, classified as the channel,
        once the channel was lowered below the turn (issue 141) or is gone. A call that did not
        run - denied, or given up while waiting for approval - is not told. A step whose calls do
        not all have their result is ended first, as on a cancel: those without one are answered as
        not executed on the dead branch, and those that ran are told (issue 148).

    A turn that produced no message leaves nothing: there is nothing to keep. `cancel` does not give
    a turn up; it takes it over - unless the turn no longer flows into its channel.

Before every step of a turn, the agent asks whether the turn still flows into its channel as it is
now (`ActionType::Publish` of the turn's classification, in the agent's name). Once the channel was
lowered below it, the turn is given up as `Lowered`, whatever it was about to do - run a tool, take
the turn over after a cancel - and the users are told with `TurnFailed` (issue 141, see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

Both end the turn as an `EndedTurn`, which sends the turn's last event - `FullResponseGenerated` or
`TurnFailed` - with the contexts and the publication the turn had.

### Storing and resuming

The state is stored after every step, together with the state that comes next (issue 85). What a
crash leaves in the store is the step still to do, not the one that is done. When the agent is
loaded again, the turn is resumed (issue 75):

*   `Planning` asks the model again with the turn so far.
*   `Reflecting` takes the finished turn over.
*   `WaitingForApproval` keeps waiting.
*   `Cancelling` ends the turn.
*   `ExecutingTools` is the exception. A tool may have run without its result being stored, and
    running it again would repeat its effect (issue 20). So every approved call without a result is
    answered as *not executed* (`prepare_resume` in `actor_impl/agent.rs`), the tool message is
    built from that, and the model decides again.

A turn state stored without the turn's working state cannot be resumed; the agent starts `Idle`.

A deliberate end never leaves a turn in the store. `cancel` and the stop tool commit the turn. An
agent removed from its channel is told to `stop`: it gives up a turn under way, stores itself idle
and ends its task. The channel aborts it only if it does not - a tool that hangs, say. The agent
announces nothing, since it has left; the channel announces its removal (`ParticipantRemoved`, see
[CHANNELS.md](CHANNELS.md)).

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
digest before it, with lookup of the prefix an earlier digest names. `Transcript` and
`MessageHistory` are that chain over a message entry each, with the methods of their content as
inherent methods on it (issue 197), and share the `message_chain` digest. A message does not name
its predecessor: the same message is in several chains with a different predecessor in each, so the
predecessor belongs to the chain, stored as `parent`, the predecessor's digest, beside the entry
([117](../issues/117-message-prev-id.md)). Notes and memory are chains too (see
[MEMORY.md](MEMORY.md)).

A chain is a tree with a head ([08](../issues/08-retry-and-edit-past-answers.md)):

*   `push_with` appends after the head and makes the new entry the head. `move_head` sets the head
    back to any entry, or to the root; what follows it stays, as a branch no longer active. Nothing
    is removed.
*   What the chain says - `digest`, `entries`, `last`, `len` - is the *active path*, from the root
    up to the head: the conversation as it stands. `all_entries` are those of every branch, for what
    has to consider all of them: looking up a message by id, fitting the history into a lowered
    classification, the next number of a note.
*   `prefix` finds an entry on any branch, so a `ContextRef` into a branch no longer active still
    resolves.
*   `children` are the siblings among which a branch is chosen, and every entry remembers the child
    last on the active path, so that `leaf_from` finds where a switch to a branch goes on.
*   An entry pushed with the digest of one already there - the same predecessor, the same content -
    is that entry: the head moves to it.
*   `amend` and `amend_each` change an entry in what its digest does not cover - a message's
    classification, the mark of a turn given up - and nothing else; there is no other way to change
    an entry. These three - append, move the head, amend - are the only changes, and the chain keeps
    a journal of them since it was last stored (`changes`, `mark_stored`, `apply`;
    [193](../issues/193-external-state-rewritten-whole.md), see [STORAGE.md](STORAGE.md)).

Giving up a turn ([145](../issues/145-ended-turns-leave-a-record.md), see [Beginning and ending a
turn](#beginning-and-ending-a-turn)) moves the heads of the transcript and the history back;
regenerating an answer (08) will move them too.

### What is recorded

`commit_turn` records a `ContextRef` with every message it appends to the history:

*   the digest of the transcript before the message,
*   the turn's prompt parts, each as a `PromptRef`,
*   the tools the model was offered, as the digest of the set (`ContextRef::tools`, see [Tools
    offered](#tools-offered)),
*   the knowledge read before the message (`ContextRef::knowledge`, see [MEMORY.md](MEMORY.md)),
*   how the model was given its context: its profile (`ContextRef::profile`), and how many of the
    transcript's last entries the turn itself had produced before the message
    (`ContextRef::turn_entries`) - see [Reasoning given back](#reasoning-given-back).

`AgentChannelState::recorded_context_of(message)` finds such a reference, and the `ContextResolver`
resolves it (see [The turn's context and the resolver](#the-turns-context-and-the-resolver)) to a
`ResolvedContext`: the messages and prompt parts the model saw, in the order it was given them, each
message with its reasoning only where the model was given it, and the tools it was offered. The
channel's history identifies the agent by the message's sender.

### Tools offered

A turn offers its model the agent's tools as they were when it began: they are asked for once, in
`AgentActor::request_response`, and fixed in the turn (`TurnState::tools`, issue 137) like its
classification and publication - a resumed turn keeps them. The model is told of a tool only its
name, description and parameters - a `ToolDefinition` (`Tool::definition`); whether its calls need
approval and how far they may travel stay with the runtime, which labels the model's answer (`label`
in `actor_impl/agent.rs`) and puts its calls up for approval (`GuardedToolExecutor::admit`) by the
same `Tool`s. A tool that changes or disappears during a turn is offered as it is from the next turn
on; a call of one that is gone fails as unknown.

`Turn::next_turn` keeps the set in `AgentChannelState::tool_sets`, once per digest (`digest::tools`,
in the order offered), and puts its digest into the turn's context, so that every message of the
turn records it; no tools are recorded as `None`. `LLMClient::chat_completion` gets the tools from
the resolved context, like the messages, so what the model was offered and what is recorded as
offered are the same.

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

The profile is handed to every `Turn` in `Turn::new`, from the agent's model, fixed in the context
of a turn when it begins (`Turn::next_turn`) - so that a resumed turn keeps the profile it began
with - and applied in one place: the `ContextResolver`, which resolves what the model is given - the
transcript before the turn, then the messages the turn produced (`ReasoningHistory::given`). A
backend therefore gets only what its model may see and implements no rule of its own. The stored
transcript keeps all reasoning; only the model's input leaves it out.

Since what the model was given is less than the transcript holds, the `ContextRef` records the
profile and the turn's share of the transcript, and resolving it applies the same rule again. The
digests are unchanged: they name the stored transcript, not a filtered copy of it.

### Digests are stored, not recomputed

A digest is a record of what was sent to the model, not a value to derive again. It is stored with
its entry and never recomputed. Were the scheme to change, recomputing would silently change every
digest the history refers to; stored, old entries keep their digests and only new ones get the new
scheme's.

### The context only grows

This relies on the context only ever growing, so that an earlier digest names a prefix of it. A turn
under way does not change it either: its messages are appended only when it is taken over (see [The
turn's context and the resolver](#the-turns-context-and-the-resolver)).

Compressing the context ([16](../issues/16-context-compression.md)) produces a new context that came
from the old one but does not continue it. Entries written before keep referring to the old context,
which therefore has to remain resolvable; only new entries refer to the new one. The link from the
new context to the one it came from is a relation of its own, not the predecessor link of the chain,
and - since it is not sent to the model - not part of the digest.

## Prompt parts

What the model is given besides the conversation - the system prompt, notes, what the agent
remembers - is a *prompt part* (issue 47). The `ContextProvider` decides where the parts go; the
`Turn` records what it was handed.

### The `ContextProvider`

An agent has exactly one `ContextProvider` (issue 98), a facet registered with
`ComponentRegistry::single`. A personality may configure its own, to try another way of assembling
the context; otherwise it gets the default, `state_context_provider`, or the one
`AppConfig::component_defaults` names. Two would overwrite each other's context, so a personality
with two is refused.

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

`StateContextProvider` asks for every `PromptProvider` of the agent by type (issue 99) and joins
their parts per position, in the order of the configuration. `static_system_prompt_provider` takes
its position from its parameter `position` (`start` by default, or `before_turn_input`), and how its
prompt is classified from `classification` (`public` by default). Every part is classified as its
provider says - a part that does not say counts as the channel - and what the parts of one position
are joined into, as the join of theirs; the turn has seen it (issue 168).

The parts may be left out, each by the fail policy of its own component: a part that fails open is
left out of that turn with a warning; one that fails closed fails the turn.

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
ought to reach the model at once, at the cost of the prompt cache from there on. How the senders of
messages are marked in the conversation (issue 135) is to be explained beside it, once they are.

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
`ContextResolver` (`llm_impl/control_tokens.rs`):

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
    (`TurnState::context`): the digest of the transcript so far, a `PromptRef` for every prompt
    part, the digest of the tools offered, the knowledge read, the model's profile, and how many of
    the transcript's last entries are the turn's own. References only - the turn keeps no copy of
    the transcript. `Turn::next_turn` begins it on the transcript as it stands.
*   **The turn's messages.** `Turn::push_message` gives a message the digest it will have in the
    transcript - computed as the chain would, continuing the transcript - keeps it with a copy of
    the context it was produced from (`TurnEntry`), and moves the context on past it. The messages
    go into the transcript only when the turn is taken over; a discarded turn leaves it untouched.
    Until then a `TranscriptView` continues the stored transcript with them. This relies on the
    transcript not changing while a turn runs - nothing is perceived during one - and `commit_turn`
    logs it if it did.
*   **Placing the parts.** The `ContextProvider` calls `Turn::set_context` with every part as a
    `PromptPart`: its text, its role and the transcript entry it follows. An instruction within the
    turn is placed after its last message (`Turn::place_turn_prompt`). Either way the text is kept
    in `AgentChannelState::prompts`, once per digest, and the reference goes into the turn's
    context, where it counts for the messages produced from then on - like knowledge read.
*   **Resolving.** The `ContextResolver` resolves a `ContextRef` against the view, the prompt texts
    and the tool sets: the transcript it names, of which the last `turn_entries` are the turn's own,
    with every part after its entry, each as the profile gives it, and the tools offered.
    `Turn::full_context` resolves the turn's context and gives the model the result, a prompt part
    as a message in its role.
*   **Recording.** `commit_turn` appends the turn's messages to the transcript and records with each
    the context it was produced from. The same resolver resolves that `ContextRef` when what the
    agent saw is asked for.

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
    (`view::member`: a user, an agent, an approver, one that is gone). An agent's reads its
    personality when it is first needed, not for every member looked at, and hands out an
    `AgentMember` with its state in the channel as its kind keeps it - one run here, an external
    one. An agent whose personality is gone has no state to read, as it is not run in the channel
    (issue 189). What the viewer may see of it, the member view decides for all of them alike, from
    the agent's document. The
    `AgentContextView` it answers with - a trait, with a view per kind - holds that state: the
    transcript, the private notes, and
*   `context_of(message_id)`, the context and the prompt parts the message was produced from
    (`ContextError::NotRecorded` if the agent did not produce it, `Unavailable` if what was recorded
    cannot be resolved any more), and
*   `given_up_turns()`, the turns the agent gave up in the channel (issue 145): each with its
    reason, time and messages. The context of each of those messages is asked for like that of any
    other.

Of an external agent (issue 179) the view reads its own state (`ExternalAgentChannelState`): its
transcript is what the remote agent was given, it has no notes, no private notes, no given-up turns
and no pending approvals, and `context_of` answers `NotRecorded` for every message - what the remote
agent made of what it was given is its own.

REST exposes it as `GET /sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context`. It
answers `403 context_forbidden`, `404 context_not_recorded` (not produced by that actor) or `404
context_unavailable` (recorded, but no longer resolvable). The response lists the knowledge read
with channel and agent names - a channel of another session is not named - and the tools offered
with the digest of their set. Every prompt part and every knowledge read carries its
`classification` (`null` if not known), and every item and knowledge read `sets_label`: whether it
set the message's label (issue 170, `LabelCauses` in the REST server; the rule is in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md)). `GET …/actors/{aid}/given-up-turns` lists the turns given
up, oldest first, and answers `403 context_forbidden` alike.

So that a client offers this only where it works, every member of a channel response carries
`can_read_context`, decided for the requesting user when the channel's snapshot is taken (see
[SECURITY.md](SECURITY.md#views)): for agents only, the same policy question `MemberView::context`
asks, whether the channel runs or not.

The web UI shows it in its context panel (issue 41), with the prompt parts where they were given,
the knowledge read and the tools offered; see [WEB-UI.md](WEB-UI.md#what-an-agent-saw).
