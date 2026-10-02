# Memory

What an agent remembers beyond the turn it is in: the notes it publishes in a channel, its private
notes, and its memory across channels and sessions; how it reads what other channels know; and how
every read is recorded. How a turn takes over what it staged is described in
[AGENT-TURN.md](AGENT-TURN.md); what may travel between channels in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md). The principles behind all of it are recorded in
[19](../issues/19-memory-tiers.md); the words are defined under [Terms](#terms) below.

## Common rules

All three kinds of memory work alike:

*   **Chains that are only appended to.** Each is a chain of changes on the generic `Chain` type
    (see [AGENT-TURN.md](AGENT-TURN.md#the-transcript-as-a-chain)): entries "note" or "revoke N",
    "remember" or "forget N". What is remembered now is the chain's active branch read from start to
    end; a number, once given, is not given again on any branch. Notes branch with the conversation,
    agent memory does not (issue 19, point 7).
*   **Only the author changes them.** An agent notes, revokes, remembers and forgets only its own
    entries.
*   **Commit, don't write.** A tool stages its change on the turn; the change is applied by
    `Turn::commit_turn` when the turn completes, and a turn that fails leaves none (issue 65).
*   **The logic is a wrapper around the turn.** `Turn` knows nothing of notes or memory.
    `agent_notes::AgentNotes` and `agent_memory::AgentMemory` hold the logic, and the components use
    them.
*   **Every read is recorded** as knowledge (see [Recording what was
    read](#recording-what-was-read)).

## Notes

The `notes` component (issue 17) lets an agent note what it wants to remember in a channel. Its
tools `note` and `revoke_note` change the agent's own notes; its prompt part gives the model the
notes of every agent in the channel. Notes belong to the agent that wrote them and are published in
the channel.

*   **Where they live.** Each agent keeps its notes per channel in `AgentChannelState::notes`, a
    `NoteChain`. Every entry records the message it follows and, as its `NoteOrigin`, the digest of
    the agent's transcript.
*   **A ledger.** `NoteChain` and `MemoryChain` are one type, `caelumora_model::ledger::Ledger<K>`
    (issue 92): a chain of `Change`s - `Add`, `Remove`, `Reclassify` - read from start to end into
    the `Item`s that stand now. The `LedgerKind` (`Notes`, `Memories`) says only what differs: the
    type of the numbers, where a change is recorded to have been made (`Origin`), and the digest's
    scheme and whether the origin is part of it. Revoking a note and forgetting a memory entry are
    both a `Remove`.
*   **Numbers.** Notes are numbered per agent, counting up, and a number is never given again, so
    that a model can repeat it to revoke.
*   **Staging.** `AgentNotes` numbers, notes, revokes and reclassifies (`NoteKind::Public` or
    `Private`). A change is staged as an `AgentChannelChange` (`Turn::stage_channel_change`) and
    applied generically through `AgentChannelState::apply` when the turn completes, after the turn's
    last message in the history.
*   **What the turn sees.** Each operation looks at the notes as they stand with the turn's own
    changes in them, `AgentNotes::effective` - the ledger's fold run over the staged changes as well
    (`Ledger::with_staged`, issue 156). A note revoked in the turn that noted it is revoked like any
    other: both changes are taken over, and its number is used up.
*   **Announcing.** The agent then stores its channel state and announces
    `GlobalEvent::AgentChannelStateChanged` - with the channel's attributes, without content -
    before `FullResponseGenerated` ends the turn. Whoever reacts to the end of the turn thus reads
    the state it left.
*   **Reading.** Others read published notes from the agents' states in the `StateStore`
    (`ChannelView::notes`), so only notes of finished turns are ever seen.
*   **Prompt part.** The notes prompt part is a `BeforeTurnInput` part, empty when nothing is noted:
    the agent's own notes first, then those of the other agents of the channel.
*   **Clients.** The channel snapshot carries `notes` per agent, and `GET …/channels/{cid}/notes`
    reads them again; the web UI does so when the global stream announces `AgentChannelStateChanged`
    for the channel (see [WEB-UI.md](WEB-UI.md#members-moderator-and-notes)).

## Private notes

The `private_notes` component (issue 51) gives an agent a second chain per channel,
`AgentChannelState::private_notes`, with its own tools `note_private` and `revoke_private_note`.

*   **Own numbering.** Private notes are numbered separately, so that gaps in the public numbers do
    not reveal them.
*   **Staging.** Changes are staged and taken over with the turn like public ones
    (`AgentChannelChange::PrivateNote`).
*   **Prompt part.** Its `BeforeTurnInput` part shows the agent its private notes of this channel
    and - through the turn's views, `MemberView::private_notes` in each - of every other channel of
    the session that this channel may read (`AccessQuestions::may_channel_read`, the same rule as
    for [reading another channel](#reading-other-channels)). A note carries [the turn's
    label](CONFIDENTIALITY.md#what-a-turn-writes-as) - the classification of what the turn had seen,
    at most its channel's - stamped when it is written, and is read note by note;
    `reclassify_private_note` changes it (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reclassification)).
*   **Hidden calls.** Both tools are `ToolVisibility::HIDDEN`, so the calls never reach the channel.
    "Private" means private from other agents and other users, not from the agent's owner, who sees
    them in the context panel, which is theirs alone (issue 54).

## Agent memory

The `agent_memory` component (issue 62) lets an agent remember what it should know across its
channels and, for agents of user or global scope, across sessions. It is an alternative to private
notes, chosen per personality ([68](../issues/68-notes-in-agent-state.md)).

*   **Where it lives.** The memory is a `MemoryChain` in the agent's own `AgentState`
    (`AgentState::memory`), not in one channel. It does not branch with a channel's history. Every
    change records its `MemoryOrigin`: the channel it was made in and the privacy of that channel's
    session then; an entry stands with the origin it was added at.
*   **Tools.** `remember(text)` and `forget(id)`, both `ToolVisibility::HIDDEN`: like private notes,
    memory is the agent's own, seen by its owner in the context panel only.
*   **Where it is recalled.** Every entry carries the label of the turn that wrote it - the
    classification of what the turn had seen, at most its channel's
    ([CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-a-turn-writes-as), issue 168) - and is recalled
    only where that may be read. The component asks the policy in the agent's name
    (`AccessQuestions::may_channel_read` through the `SecurityManager` of its `ComponentContext`,
    issue 131), once per channel the entries come from. Under the ownership policy an entry is
    recalled where its own classification - the turn's label when it was written, unless it was
    reclassified - flows into the current channel's (the flow rule of issue
    133) - within the session, and beyond it only where the entry's session permits it
         ([134](../issues/134-session-privacy.md)). An entry keeps the privacy its session had when
         it was written (`MemoryOrigin::session`); in another session it is recalled only where both
         that and the privacy the session has now permit it, so that what was said under a private
         policy stays private and a session made private again takes back what it let go. The
         current one is looked up through the turn's views (`Views::privacy_of`), in the runtime's
         name - not an edge the agent could follow; of a session that is gone, the one kept decides,
         and one that cannot be looked up recalls nothing. A turn in no session - in tests - recalls
         nothing. Only entries recalled in a channel, or remembered in the turn, can be forgotten
         there, or reclassified (`reclassify_memory`, see
         [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reclassification)): `AgentMemory::find` looks the
         entry up in the memory as it stands with the turn's changes (`AgentMemory::effective`,
         issue 156) and asks the policy about that one entry.
*   **Prompt part.** A `BeforeTurnInput` part lists what may be recalled in the channel, as the
    memory is now.
*   **Staging.** A change is staged as an `AgentStateChange` (`Turn::stage_agent_change`).
    `commit_turn` applies it to the agent's state as it is when the turn completes, under the
    handle's lock, stores it and announces `GlobalEvent::AgentStateChanged`. Actors of the same
    agent in other channels and sessions thus neither lose nor overwrite each other's changes (see
    [STORAGE.md](STORAGE.md)).
*   **Numbers.** Unlike a note's, the number of a new entry is reserved at once on the agent's
    shared state handle (`AgentStateHandle::reserve_memory_id`), since turns in several channels may
    remember at the same time. This is deliberately the one exception to "commit, don't write"
    (issue 19).

## Reading other channels

Components read what other channels know through the views of the turn they are called with
(`Turn::session`, issue 163; see [SECURITY.md](SECURITY.md#views)): the agent's session, from which
they reach its channels (`SessionView::channels`, `channel_named`), their notes and histories
(`ChannelView::notes`, `history`) and the agent's own private notes there
(`MemberView::private_notes`). A turn outside of a channel - in tests - has no session and reads
nothing.

*   **From the own channel.** The agent reads as a viewer *from a place* (`Viewer::from_place`, a
    `Place`): its channel, the classification its turn began with (issue 67) and its session's
    privacy. The views are built once per turn from the actor's runtime, which reaches the session
    through its channel (`ChannelLoop::session_loop()`, a weak handle the `SessionLoop` sets on each
    of its channels with `ChannelLoop::attach`, issue 123).
*   **Who may read what.** Another channel is read as a whole if the agent's channel may read it by
    their classifications (`AccessQuestions::may_channel_read`, `ActionType::ReadChannel`) - those
    are what `SessionView::channels` lists; the rest (`all_channels`, `channel_named`) entry by
    entry - message, note - as each entry's classification may be read. That depends on the channels
    and the entry, not on who takes part in them, because an agent passes on what it reads to its
    channel; an entry released below its channel is read where the channel as a whole is not (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).
*   **No probing.** A channel that does not exist and one that may not be read, and of which nothing
    was released, are the same error (`ViewError::NoChannelNamed`), so channels cannot be probed. A
    tool passes it on with `?` (`From<ViewError> for ToolError`).
*   **Tools.** `read_notes(channel?)` in `notes` reads the notes of another channel, or without a
    channel of every channel readable as a whole. `read_history(channel, last?, query?)` in the
    component `channel_history` reads another channel's conversation.

## Recording what was read

Every read is recorded as a `KnowledgeRef`: the channel and its session (so that knowledge from
another session can be told, issue 134), and what was read there (`KnowledgeSource`):

*   `Notes { agent, digest }` - the notes an agent published in the channel;
*   `PrivateNotes { agent, digest }` - the agent's own private notes in the channel;
*   `History { digest, start }` - the channel's history: up to where, and, for a reader who joined
    later with a limit, from where (issue 72);
*   `Memory { agent, digest }` - the agent's memory, recorded once for every channel the entries
    recalled were written in.

`Turn::record_knowledge` records a read together with the number of messages the turn had produced
by then. `commit_turn` puts it into the `ContextRef` of every message produced after it
(`ContextRef::knowledge`). A `PromptProvider` returns a `Prompt` with its text and the knowledge it
contains, so prompt parts record their reads as well.

The message context over REST lists the knowledge with channel and agent names; the context panel
shows it as "Knowledge read" (see [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw)). A turn
that read private notes or memory has seen private knowledge (`Turn::saw_private_knowledge`), which
makes its reasoning private (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

## Terms

The words for what an agent remembers are used in one sense each (issue 19, where the principles
behind them are recorded). "Branches" means: follows the branch of the conversation it came from
([08](../issues/08-retry-and-edit-past-answers.md)).

| Term | Meaning | Scope | Lifetime | Branches |
|---|---|---|---|---|
| **History** | The record of what happened, never rewritten or compressed: per channel (`ChannelState`), and per agent in a channel with what each own message was produced from. | channel / agent in channel | session | yes |
| **Transcript** | The sequence of messages an agent's context is built from (`Transcript`): its own messages in full, with tool calls and internal steps, and what others said as it received them; may later be compressed ([16](../issues/16-context-compression.md)). Not the history, which is the channel's public record ([50](../issues/50-turn-context-naming.md)). | agent in channel | session | yes |
| **Prompt part** | Text given besides the conversation - the system prompt, notes - transient, recorded by digest (see [AGENT-TURN.md](AGENT-TURN.md#prompt-parts)). | turn | transient | - |
| **Context** | What one call of the model is given, assembled on purpose: prompt parts, the transcript, the turn's messages and the tools offered - "context" in the usual LLM sense (`Turn::full_context`, placed by the agent's `ContextProvider` and resolved by the `ContextResolver`); reconstructable through `ContextRef` ("what the agent saw"). Formerly also called *model input* ([50](../issues/50-turn-context-naming.md)). | one call | transient | - |
| **Turn state** | The working state of a turn: plan, pending tool calls, the turn's messages, the changes staged to the agent's states. | agent in channel | turn | discarded |
| **Notes** | Entries in an agent's public note chain of one channel, published there, with provenance; only the author writes and revokes ([17](../issues/17-information-sharing-within-session.md)). | agent in channel | session | yes |
| **Session memory** | The union of the active public note chains of all agents in all channels of a session - a view, not a store. | session | session | per channel |
| **Agent memory** | An alternative to private notes, chosen per personality ([68](../issues/68-notes-in-agent-state.md)): what an agent remembers across its channels and sessions: a chain in its `AgentState` (component `agent_memory`, tools `remember` / `forget`), each entry carrying the label of the turn that wrote it and recalled only where that may be read ([62](../issues/62-json-store-concurrent-writes.md)). | agent | beyond the session | no |
| **Private notes** | An agent's private note chain in one channel - the same mechanism as notes, readable only by the agent, carrying the classification of that channel ([51](../issues/51-agent-private-notes.md)). | agent in channel | session | yes |
| **Agent session memory** | The union of an agent's active private note chains across its channels - a view, readable only by the agent, each note only where its channel's classification allows. | agent in session | session | per channel |
| **Agent knowledge** | What an agent keeps across sessions, committed from notes ([18](../issues/18-knowledge-sharing.md)). | agent | permanent | no |
| **Shared knowledge** | Knowledge shared by several agents or users ([18](../issues/18-knowledge-sharing.md)). | group / global | permanent | no |
| **Workspace** | Documents several channels edit together; outside the branches, every change an effect ([20](../issues/20-transactional-effects.md)). | session or beyond | until deleted | no |

Two technical words are not tiers: **state** is the container the `StateStore` keeps (history,
context, turn state and notes live *in* state), as opposed to **entities** in the `EntityStore`; and
the structs named `…Context` (`SecurityContext`, `EventContext`, `ComponentContext`,
`CommandContext`, `Turn`) are *operation contexts*, the circumstances an operation runs in,
unrelated to the model's context. In prose, "context" alone always means the model's context; the
others are named in full.
