# The Agent Turn

How an agent answers: the cycle its `AgentActor` runs, the `Turn` it works
on, how a turn survives a crash, and how it can be told afterwards what the
model saw. What an agent remembers beyond a turn - notes, private notes,
memory - is described in [MEMORY.md](MEMORY.md); the components that take
part in a turn in [COMPONENTS.md](COMPONENTS.md); tools and approvals in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

## The cognitive cycle

`AgentActor::run` is a state machine over `ActorState`. Two states wait
between turns, five belong to a turn.

**Between turns** (`Idle`, `Sleeping`) the agent waits for the next event:

*   **Perceive.** A `FullResponseGenerated` of another participant is
    passed through the agent's `PerceiveInProcessor`s
    (`transcript_appender`, `state_history_appender`), which append it to
    the transcript and the history. No turn begins, and nothing is asked of
    the policy (issues 85, 90).
*   **Begin.** A `ResponseRequested` from the moderator begins a turn
    (`Turn::next_turn`, see below), announces `ResponseStarted` and has the
    agent's `ContextProvider` assemble the context. The agent goes on in
    `Planning`. A sleeping agent ignores it; only `wakeup` ends the sleep.

**In a turn** the agent moves through these states:

*   **`Planning`** calls the model with the context and the agent's tools
    and streams the answer. Partial messages are published as they arrive.
    When the complete message contains tool calls, the agent registers them
    for approval where their tool requires it and goes to `ExecutingTools`;
    otherwise to `Reflecting`. A turn makes at most 25 model calls
    (`max_iterations`), then it goes to `Reflecting`.
*   **`ExecutingTools`** runs the approved calls that have no result yet.
    Once every call of the step has a result, the tool message goes into the
    turn and the agent returns to `Planning` - or to `Reflecting` if a tool
    asked for the turn to end, as the stop tool does. If calls are still
    pending, it goes to `WaitingForApproval`.
*   **`WaitingForApproval`** waits for `ApprovalGranted`, `ApprovalDenied`
    or `ApprovalTimedOut`, records the decision and returns to
    `ExecutingTools`. Other events stay queued until the turn is over. See
    [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md) for the approval process.
*   **`Reflecting`** takes the turn over (`Turn::commit_turn`), stores the
    state and announces the end with `FullResponseGenerated`. The agent then
    lands in the state a tool requested (`Sleeping` after `stop` or `bye`)
    or in `Idle`.
*   **`Cancelling`** is entered when the channel cancels the turn. It takes
    the turn over like `Reflecting` - so observers learn that generation
    stopped - and returns to `Idle`.

Every message that leaves the agent passes its `EmitOutProcessor`s first,
of which there is no implementation yet (issue 99), and is published
through `Turn::send_event`, which redacts it by the channel's
confidentiality (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

When a plan is in progress (`create_plan`), the messages of its steps are
*internal*: they are published as intermediate messages, kept in the
transcript, but not in the history. The answer after the last step is the
final one.

An error in one of the turn's states ends the turn, not the agent: the
agent logs it, discards the turn (`Turn::abandon_turn`), emits
`TurnFailed` and returns to `Idle` (see
[CODING-GUIDELINES.md](CODING-GUIDELINES.md)).

## The `Turn`

The agent's state in its channel is its `AgentChannelState`: its
`MessageHistory`, its `Transcript`, its notes and the texts of its prompt
parts. Its own state across channels is its `AgentState`. A step of the
cycle works on a `Turn` (`caelumora_runtime::agent_turn`), the handle to
both.

The working state of a turn is one value, `AgentChannelState::turn:
Option<TurnState>`, which is `None` between turns (issue 85). It holds what
the turn is and what it has done so far:

*   the envelope it answers (`EventContext`),
*   whose authority it acts with (`SecurityContext`),
*   the channel as the confidentiality rules see it
    (`ChannelClassification`),
*   what the `SecurityManager` lets it publish there (`Publication`),
*   the model's context, the messages produced, the knowledge read,
*   the changes staged to the agent's states,
*   the tool calls, the plan and the state a tool requested.

Work between turns does not touch it: perceiving a message only appends to
the transcript and the history.

### Beginning and ending a turn

*   **`Turn::next_turn`** begins a turn and fixes the first four of the
    values above. The classification and the publication are taken once and
    hold for the whole turn, so that what the turn reads, publishes and
    remembers is judged alike, even if the channel's level changes
    meanwhile. All four are stored with the channel state, so that a turn
    resumed after a restart is the same turn.
*   **`Turn::commit_turn`** takes a completed turn over:
    *   it appends the agent's messages to its transcript, and the
        non-internal ones to its history, each with a `ContextRef` (see
        below);
    *   it applies the changes staged to the agent's state in the channel,
        and those staged to its own state;
    *   it clears the working state, so that none of it is stored with the
        channel state.

    It returns a `CommittedTurn`: the final messages for
    `FullResponseGenerated`, whether the channel state changed, and the
    ended turn.
*   **`Turn::abandon_turn`** discards a turn that failed or was stopped.
    Nothing it produced, staged or read is taken over.

Both end the turn as an `EndedTurn`, which sends the turn's last event -
`FullResponseGenerated` or `TurnFailed` - with the contexts and the
publication the turn had.

### Storing and resuming

The state is stored after every step, together with the state that comes
next (issue 85). What a crash leaves in the store is the step still to do,
not the one that is done. When the agent is loaded again, the turn is
resumed (issue 75):

*   `Planning` asks the model again with the turn so far.
*   `Reflecting` takes the finished turn over.
*   `WaitingForApproval` keeps waiting.
*   `Cancelling` ends the turn.
*   `ExecutingTools` is the exception. A tool may have run without its
    result being stored, and running it again would repeat its effect
    (issue 20). So every approved call without a result is answered as *not
    executed* (`AgentActor::prepare_resume`), the tool message is built from
    that, and the model decides again.

A turn state stored without the turn's working state cannot be resumed; the
agent starts `Idle`.

A deliberate end never leaves a turn in the store. `cancel` and the stop
tool commit the turn. An agent removed from its channel is told to `stop`:
it abandons a turn under way, stores itself idle and ends its task. The
channel aborts it only if it does not - a tool that hangs, say.

## What an agent saw

For every message an agent produced, it can be told what its model saw
when it wrote it (issue 06).

### The transcript as a chain

The `Transcript` is the sequence of messages the agent's context is built
from: its own messages in full, with tool calls and internal steps, and
what others said as it received them. It is kept as a chain. Every entry
has a `Digest` (SHA-256, `caelumora_model::digest`) over its message and the
digest before it, so one digest names the whole transcript up to that point
by content.

What goes into a message's digest is fixed in one place,
`digest::message_chain`, behind a version tag:

*   included: id, role, sender, content, reasoning, tool calls and results,
    and whether the message is internal;
*   not included: timestamps, usage statistics, resolved mentions and
    metadata.

JSON values such as tool arguments are hashed with sorted keys, so that the
digest depends neither on how a value was built nor on `serde_json`
features.

The chain is one generic type, `caelumora_model::chain::Chain<E>` (issue
48): entries that are only appended to, each storing the digest its type
computed over its content and the digest before it, with lookup of the
prefix an earlier digest names. `Transcript` and `MessageHistory` are
chains of messages and share the message-specific part: the
`message_chain` digest, and `prev_id`, which each chain sets to its own
last message - so the same message has different predecessors in different
chains. Notes and memory are chains too (see [MEMORY.md](MEMORY.md)), and
branching ([08](../issues/08-retry-and-edit-past-answers.md)) will turn the
type into a tree.

### What is recorded

`commit_turn` records a `ContextRef` with every message it appends to the
history:

*   the digest of the transcript before the message,
*   the turn's prompt parts, each as a `PromptRef`,
*   the knowledge read before the message (`ContextRef::knowledge`, see
    [MEMORY.md](MEMORY.md)),
*   which reasoning the model was given: the rule (`ContextRef::reasoning`)
    and how many of the transcript's last entries the turn itself had
    produced before the message (`ContextRef::turn_entries`) - see
    [Reasoning given back](#reasoning-given-back).

`AgentChannelState::seen_context_of(message)` resolves such a reference to
the messages and prompt parts the model saw, in the order it was given
them, each message with its reasoning only where the model was given it.
The channel's history identifies the agent by the message's sender.

### Reasoning given back

Which reasoning of earlier messages a model is given again is a property of
the model (issue 63): some must not see earlier reasoning, others need the
reasoning that led to a tool call within the same turn. The agent's
`LLMClient` declares it as a `ReasoningHistory`
(`caelumora_model::message`):

| Rule | The model is given |
|---|---|
| `never` | no reasoning at all |
| `current_turn` | the agent's own reasoning of the turn under way - the default |
| `always` | the agent's own reasoning of every turn |

Whatever the rule, a model is never given another actor's reasoning.

The rule is handed to every `Turn` in `Turn::new`, from the agent's model,
and applied in one place: `Turn::full_context`, which assembles what the
model is given - the context the turn began with, then the messages the
turn produced (`ReasoningHistory::given`). A backend therefore gets only
what its model may see and implements no rule of its own. The stored
transcript keeps all reasoning; only the model's input leaves it out.

Since what the model was given is less than the transcript holds, the
`ContextRef` records the rule and the turn's share of the transcript, and
`resolve` applies the same rule again. The digests are unchanged: they name
the stored transcript, not a filtered copy of it.

### Digests are stored, not recomputed

A digest is a record of what was sent to the model, not a value to derive
again. It is stored with its entry and never recomputed. Were the scheme to
change, recomputing would silently change every digest the history refers
to; stored, old entries keep their digests and only new ones get the new
scheme's.

### The context only grows

This relies on the context only ever growing, so that an earlier digest
names a prefix of it. Compressing the context
([16](../issues/16-context-compression.md)) produces a new context that
came from the old one but does not continue it. Entries written before keep
referring to the old context, which therefore has to remain resolvable;
only new entries refer to the new one. The link from the new context to the
one it came from is a relation of its own, not the predecessor link of the
chain, and - since it is not sent to the model - not part of the digest.

## Prompt parts

What the model is given besides the conversation - the system prompt,
notes, what the agent remembers - is a *prompt part* (issue 47). The
`ContextProvider` decides where the parts go; the `Turn` records what it was
handed.

### The `ContextProvider`

An agent has exactly one `ContextProvider` (issue 98), a facet registered
with `ComponentRegistry::single`. A personality may configure its own, to
try another way of assembling the context; otherwise it gets the default,
`state_context_provider`, or the one `AppConfig::component_defaults` names.
Two would overwrite each other's context, so a personality with two is
refused.

The `ContextProvider` runs once, at the start of a turn, so a part stays in
place across the tool loop. When it fails, the turn fails, whatever its
fail policy: without a context there is nothing to give the model.

### Parts and their position

A `PromptProvider` contributes a part with a `PromptPosition`:

*   **`Start`** - the system prompt. It goes before everything, as a
    system message.
*   **`BeforeTurnInput`** - goes transiently before the newest message of
    the turn, as a user message; at the end, if the agent wrote that
    message itself, in a turn without new input.

The distinction exists because many models cache the prompt up to the
first change, and some merge every system message into the system prompt
at the front. Information that changes often therefore does not belong in
the system prompt. A part is never stored in the transcript, which stays a
stable prefix.

`StateContextProvider` asks for every `PromptProvider` of the agent by type
(issue 99) and joins their parts per position, in the order of the
configuration. `static_system_prompt_provider` takes its position from its
parameter `position` (`start` by default, or `before_turn_input`).

The parts may be left out, each by the fail policy of its own component: a
part that fails open is left out of that turn with a warning; one that
fails closed fails the turn.

### How parts are recorded

The provider hands the turn a `Vec<ContextItem>`: transcript entries with
their digests, and prompt parts with the role they are given as.

*   `Turn::set_context` keeps every part's text in
    `AgentChannelState::prompts`, once per digest, and stores the items as
    the turn's context (`TurnState::context`).
*   What the model is sent is derived from those items: `Turn::context`
    gives each part as a message in its role, and the turn's own messages
    follow.
*   So is the `PromptRef` of each part that `commit_turn` records: the
    text's digest, its role, and the digest of the transcript entry it
    followed (`None`: before everything).

What the model was given and what is recorded as given therefore cannot
diverge. `AgentChannelState::resolve` inserts the parts after their entries
again.

## Asking what an agent saw

Asking for a message's context is an operation of the actor that produced
the message, not of the channel: the same message is in every
participant's history, but only one of them produced it.

*   `Actor::message_context(caller, message_id)` answers with the context
    and the prompt parts. Only `AgentActor` has one; every other actor
    answers `NotRecorded`.
*   The agent asks the `SecurityManager` with
    `ActionType::ReadAgentContext` and its own attributes. Under the
    ownership policy only the agent's owner may, since a context holds more
    than the conversation: the system prompt, internal steps, tool results.
*   It reads its state from the `StateStore`, where it is written after
    every step; the running loop holds the in-memory state.

REST exposes it as
`GET /sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context`. It
answers `403 context_forbidden`, `404 context_not_recorded` (not produced
by that actor) or `404 context_unavailable` (recorded, but no longer
resolvable). The response lists the knowledge read with channel and agent
names.

So that a client offers this only where it works, every member of a channel
response carries `can_read_context`, decided for the requesting user when
the runtime builds the channel's view (see [SECURITY.md](SECURITY.md)): for
agents only, the same policy question `message_context` checks, whether the
channel runs or not.

The web UI shows it in its context panel (issue 41), with the prompt parts
where they were given and the knowledge read; see
[WEB-UI.md](WEB-UI.md#what-an-agent-saw).
