# Memory

What an agent remembers beyond the turn it is in: the notes it publishes in a channel, its private
notes, and its memory across channels and sessions; how it reads what other channels know; and how
every read is recorded. How a turn takes over what it staged is described in
[AGENT-TURN.md](AGENT-TURN.md); what may travel between channels in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md). The principles behind all of it are recorded in
[19](../issues/19-memory-tiers.md); the words are defined under [Terms](#terms) below.

## Common rules

All three kinds of memory work alike:

*   **Ledgers that are only appended to.** Each is a chain of changes - an entry added, updated,
    removed or reclassified - on the generic `Chain` type (see
    [AGENT-TURN.md](AGENT-TURN.md#the-transcript-as-a-chain)), one type for both,
    `caelumora_model::ledger::Ledger`. What stands now is the active branch read from start to end.
    A number, once given, is not given again on any branch, and an entry updated keeps its own.
    Notes branch with the conversation, agent memory does not.
*   **Only the author changes them.** An agent notes, updates, revokes, remembers and forgets only
    its own entries.
*   **Commit, don't write.** A tool stages its change on the turn; the change is applied when the
    turn is taken over, and a turn that fails leaves none.
*   **What the turn sees** is the entries as they stand with the turn's own staged changes in them.
    An entry revoked in the turn that wrote it is revoked like any other: both changes are taken
    over, and its number is used up.
*   **The logic is a wrapper around the turn.** `Turn` knows nothing of notes or memory;
    `AgentNotes` and `AgentMemory` hold the logic, and the components use them.
*   **Every read is recorded** as knowledge (see [Recording what was
    read](#recording-what-was-read)).
*   **Written without approval.** What an entry says may rest on a claim the model did not verify -
    a participant's message posing as another's - and stands in the agent's name from then on; see
    [APPROVALS.md](APPROVALS.md#what-needs-approval).
*   **Shown alike.** The web UI shows a tool call on one entry as the entry's number, what was done
    to it and its new text where there is one - not the text of an entry revoked, forgotten or
    reclassified.

## Notes

The `notes` component lets an agent note what it wants to remember in a channel. Its tools
`write_note`, `update_note` and `revoke_note` change the agent's own notes; its prompt part gives
the model the notes of every agent in the channel. Notes belong to the agent that wrote them and are
published in the channel.

*   **Where they live.** Each agent keeps its notes per channel, in its `AgentChannelState`. Every
    entry records the message it follows and the digest of the agent's transcript.
*   **Updating** replaces the text of an entry that stands, which keeps its number and its place.
    The new text carries the label of the turn that wrote it, as a new entry does: a release of the
    old text does not carry over. The entry stands where it was updated.
*   **Numbers.** Notes are numbered per agent, counting up, and a number is never given again, so
    that a model can repeat it to update or revoke.
*   **Changed where they were written.** The tools change notes of the turn's channel only; the
    chain of another channel is written by the agent's actor there. A number names a note in one
    chain, so numbers are shown only for notes of the turn's channel: notes of other channels are
    listed without them, public and private alike.
*   **Taken over after the answer.** Staged changes are pushed into the chain when the turn is taken
    over, after the turn's last message in the history. The agent then stores its channel state and
    announces `AgentChannelStateChanged` - without content - before `TurnFinished` ends the turn, so
    that whoever reacts to the end of the turn reads the state it left.
*   **Reading.** Others read published notes from the agents' stored states, so only notes of
    finished turns are ever seen.
*   **Prompt part.** A `BeforeTurnInput` part, empty when nothing is noted: the agent's own notes
    first, then those of the other agents of the channel.
*   **Clients** get the notes per agent with the channel's snapshot and read them again when the
    global stream announces a change (see [WEB-UI.md](WEB-UI.md#members-moderator-and-notes)).

## Private notes

The `private_notes` component gives an agent a second chain per channel, with its own tools
`write_private_note`, `update_private_note` and `revoke_private_note`, staged and taken over like
public notes.

*   **Own numbering.** Private notes are numbered separately, so that gaps in the public numbers do
    not reveal them.
*   **Prompt part.** Its `BeforeTurnInput` part shows the agent its private notes of this channel
    and of every other channel of the session it takes or took part in, as far as they may be copied
    into this one (see [reading other channels](#reading-other-channels)). It reaches them through
    the agent itself, not through the channels. A note carries [the turn's
    label](CONFIDENTIALITY.md#what-a-turn-writes-as), stamped when it is written, and is read note
    by note; `reclassify_private_note` changes it (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reclassification)).
*   **Hidden calls.** The tools are hidden, so their calls never reach the channel. "Private" means
    private from other agents and other users, not from the agent's owner, who sees them in the
    context panel and - from every channel of the session they may read - in the private notes
    panel (see [WEB-UI.md](WEB-UI.md)).

## Agent memory

The `agent_memory` component lets an agent remember what it should know across its channels and,
for agents of user or global scope, across sessions. It is an alternative to private notes, chosen
per personality.

*   **Where it lives.** The memory is a chain in the agent's own `AgentState`, not in one channel,
    and does not branch with a channel's history. Every change records where it was made: the
    channel, and the privacy of that channel's session then.
*   **Tools.** `remember(text)`, `update_memory(id, text)` and `forget(id)`, all hidden: like
    private notes, memory is the agent's own, seen by its owner in the context panel only.
*   **Where it is recalled.** Every entry carries the label of the turn that wrote it
    ([CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-a-turn-writes-as)) and is recalled only where that
    may be read. The component asks the policy (`may_copy_information`) in the context of the turn
    it is called in - the agent's, with its owner's rights - once per channel the entries come
    from. Under the ownership policy an entry is recalled where its classification flows into the
    current channel's - within the session, and beyond it only where the entry's session permits
    it. In another session an entry is recalled only where both the privacy its session had when it
    was written and the privacy that session has now permit it, so that what was said under a
    private policy stays private and a session made private again takes back what it let go. The
    current privacy is looked up in the runtime's name, not along an edge the agent could follow;
    of a session that is gone, the one kept decides, and one that cannot be looked up recalls
    nothing.
*   **Changed where recalled.** Only entries recalled in a channel, or remembered in the turn, can
    be updated, forgotten or reclassified there (`reclassify_memory`, see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reclassification)); the policy is asked about that one
    entry.
*   **Prompt part.** A `BeforeTurnInput` part lists what may be recalled in the channel, as the
    memory is now.
*   **Taken over on the current state.** A staged change is applied to the agent's state as it is
    when the turn is taken over, under the state handle's lock, stored and announced
    (`AgentStateChanged`). Actors of the same agent in other channels and sessions thus neither lose
    nor overwrite each other's changes (see [STORAGE.md](STORAGE.md)).
*   **Numbers.** Unlike a note's, the number of a new entry is reserved at once on the agent's
    shared state, since turns in several channels may remember at the same time. This is
    deliberately the one exception to "commit, don't write".

## Reading other channels

Components read what other channels know through the views of the turn they are called with
(`Turn::session`, see [SECURITY.md](SECURITY.md#views)): the agent's session, its channels and their
notes and histories, and the agent's own private notes of the other channels.

*   **From the own channel.** The agent reads as a viewer *from a place*: its channel, the
    classification its turn began with and its session's privacy.
*   **Who may read what.** Another channel is reached if the agent's owner may read it or the agent
    takes part in it itself (`GetChannel`), as a user reaches a channel they may read. Of a channel
    reached, the agent reads it as a whole if it may be copied into its own by their
    classifications (`may_copy_information`); otherwise entry by entry - message, note - as each
    entry's classification may be copied. An entry released below its channel is read where the
    channel as a whole is not (see
    [CONFIDENTIALITY.md](CONFIDENTIALITY.md#reading-another-channel)), but no release reaches an
    agent that may not reach its channel.
*   **What the agent knows itself.** Its private notes of the session's other channels and its
    memory are reached through the agent, not through the channels: no `GetChannel` is asked, each
    entry is copied by the classifications alone. It heard or wrote them, and keeps them whoever
    takes part in those channels now - also of a channel it left and its owner may not read. A user
    who looks at an agent's private notes reaches them through the channels, as far as they may read
    them.
*   **No probing.** A channel that does not exist and one that may not be read, and of which nothing
    was released, are the same error, so channels cannot be probed.
*   **Tools.** `read_notes(channel?)` in `notes` reads the notes of another channel, or without a
    channel of every channel readable as a whole. `read_history(channel, last?, query?)` in
    `channel_history` reads another channel's conversation.

## Recording what was read

Every read is recorded as a `KnowledgeRef`: the channel and its session, so that knowledge from
another session can be told, and what was read there (`KnowledgeSource`) with its digest - an
agent's notes or the agent's own private notes in the channel, the channel's history up to where and
from where, the agent's memory once per channel its recalled entries were written in, a file of the
session's workspace, a skill loaded into the turn.

A read is recorded with the turn (`Turn::record_knowledge`) and goes into the `ContextRef` of every
message produced after it. A `PromptProvider` returns the knowledge its part contains with the part,
so prompt parts record their reads as well. What was read is shown with the context of a message
(see [AGENT-TURN.md](AGENT-TURN.md#asking-what-an-agent-saw)). A turn that read private notes or
memory has seen private knowledge, which makes its reasoning private (see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md)).

## Terms

The words for what an agent remembers are used in one sense each. "Branches" means: follows the
branch of the conversation it came from.

| Term | Meaning | Scope | Lifetime | Branches |
|---|---|---|---|---|
| **History** | The record of what happened, never rewritten or compressed: per channel, and per agent in a channel with what each own message was produced from - every message said, a plan's steps and a turn given up too, how each turn ended, the commands an agent carried out. | channel / agent in channel | session | yes |
| **Transcript** | The sequence of messages an agent's context is built from: its own messages in full, with tool calls and internal steps, and what others said as it received them; may be compacted. Not the history, which is the channel's public record. | agent in channel | session | yes |
| **Prompt part** | Text given besides the conversation - the system prompt, notes - transient, recorded by digest (see [AGENT-TURN.md](AGENT-TURN.md#prompt-parts)). | turn | transient | - |
| **Context** | What one call of the model is given: prompt parts, the transcript and the tools offered - "context" in the usual LLM sense; reconstructable through `ContextRef` ("what the agent saw"). | one call | transient | - |
| **Turn state** | The working state of a turn: plan, pending tool calls, the changes staged to the agent's states. | agent in channel | turn | discarded |
| **Notes** | Entries in an agent's public note chain of one channel, published there, with provenance; only the author writes and revokes. | agent in channel | session | yes |
| **Session memory** | The union of the active public note chains of all agents in all channels of a session - a view, not a store. | session | session | per channel |
| **Agent memory** | An alternative to private notes, chosen per personality: what an agent remembers across its channels and sessions, each entry recalled only where its label may be read. | agent | beyond the session | no |
| **Private notes** | An agent's private note chain in one channel - the same mechanism as notes, readable only by the agent. | agent in channel | session | yes |
| **Agent session memory** | The union of an agent's active private note chains across its channels - a view, readable only by the agent, each note only where its classification allows. | agent in session | session | per channel |
| **Agent knowledge** | What an agent keeps across sessions, committed from notes - planned ([18](../issues/18-knowledge-sharing.md)). | agent | permanent | no |
| **Shared knowledge** | Knowledge shared by several agents or users - planned ([18](../issues/18-knowledge-sharing.md)). | group / global | permanent | no |
| **Workspace** | Documents several channels edit together; outside the branches, every change an effect ([20](../issues/20-transactional-effects.md)). | session or beyond | until deleted | no |

A **skill** is no tier of memory ([SKILLS.md](SKILLS.md)): not what an agent knows but how a task is
done, a document a user owns and edits, which an agent loads into a turn - where it stands in the
transcript as any message does - and may write for its owner.

Two technical words are not tiers: **state** is the container the `StateStore` keeps (history,
context, turn state and notes live *in* state), as opposed to **entities** in the `EntityStore`; and
the structs named `…Context` (`SecurityContext`, `EventContext`, `ComponentContext`,
`CommandContext`, `Turn`) are *operation contexts*, the circumstances an operation runs in,
unrelated to the model's context. In prose, "context" alone always means the model's context; the
others are named in full.
