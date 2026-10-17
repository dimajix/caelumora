# 19 — Memory tiers

**Status:** done
**Area:** Context and knowledge
**Summary:** The four memory tiers from `TODO.md` are named but undefined — scope, lifetime, readers and invalidation are all open.

## The problem

`TODO.md` lists four kinds of memory without defining any of them:

- agent channel context
- agent session memory
- agent global knowledge
- global knowledge

None exist. More importantly, the boundaries between them do not exist either — what belongs in
which tier, how long each lives, who can read it, and what happens at the boundaries (session ends,
agent is deleted, channel closes).

## Why it matters

This is the scope model that [17](17-information-sharing-within-session.md) and
[18](18-knowledge-sharing.md) both need. Building either of those first means inventing the tiers
implicitly and inconsistently, and then having to reconcile them.

It is also the issue where getting the *names* right pays off: "context", "memory", "knowledge" and
"history" are already used loosely across the codebase and the architecture document, and a tier
model that reuses those words imprecisely will confuse every later reader.

## Considerations

- The four tiers are distinguished by two independent axes — scope (channel / session / agent /
  global) and lifetime (turn / session / permanent). Writing them out as a grid is likely to show
  that four is either too few or too many.
- Each tier needs an answer to: what writes it, what reads it, when is it invalidated, who may see
  it, and where is it stored (`StateStore`, `EntityStore`, or something new — see
  [04](04-postgres-state-store.md)).
- "Global knowledge" is the tier most in need of justification: knowledge shared across all agents
  and all users is also the tier where one bad entry affects everything, and where
  [12](12-user-and-agent-permissions.md) has the most to say.
- Deletion cascades from [07](07-session-deletion-audit.md) land here: does deleting a session
  remove what the agent learned from it?
- Prefer expressing the tiers through the existing component traits over introducing a parallel
  mechanism.

## Principles: branching and provenance (decided 2026-09-25)

Decided while planning branching conversations ([08](08-retry-and-edit-past-answers.md)). They apply
to every tier and come before defining the tiers themselves, because they cannot be added
afterwards.

1. **A third axis: bound to a branch or not.** Besides scope and lifetime, every tier is either
   bound to the branch of the conversation it came from or independent of it. The rule: *what is
   derived from the conversation branches with it; what concerns the world does not.* From easiest
   to hardest to undo:
   - what was said (history, context, turn state): free, it lives in the message tree;
   - what was derived from it inside the session (notes,
     [17](17-information-sharing-within-session.md)): bound to the branch through its provenance;
   - knowledge that outlives the session ([18](18-knowledge-sharing.md)): only through explicit
     retraction, with limits;
   - effects outside the system ([20](20-transactional-effects.md)): not at all.
2. **Notes belong to the agent that wrote them and are published in a channel.** Every agent keeps,
   per channel it takes part in, a chain of *public notes* (and one of private notes, point 12):
   entries only appended, each with a digest over its predecessor, stored in its `AgentChannelState`
   next to its history and context. The agent is the only writer; the channel is where the notes
   were published, which decides the branch they belong to and the confidentiality they carry. The
   chains branch like the message tree: switching branches switches the notes, and a regenerated
   answer does not see what the abandoned one noted.
3. **Others read the active chains directly.** No separate step to share. Notes are taken over into
   the agent's state only when its turn completes, so only notes of finished turns are ever visible.
   The notes of a channel are the active public chains of all agents in it; *session memory* is not
   a store of its own but a view: the active public chains of all agents in all channels of the
   session. Everyone can read everything, but every entry has exactly one writer, so there are no
   write conflicts. 3a. **Revoking is part of the chain.** An agent revokes one of its entries by
   appending "revoke N"; a new version is a revocation plus a new entry. The current state is the
   active chain read from start to head. Only the author may revoke, and only entries on its active
   path. A revocation branches like everything else: forking before it makes the entry valid again
   in the new branch. The digest of the head covers revocations, so a reader's reference also names
   what had been revoked when it read.
4. **Reading across channels is an effect.** For the channel that wrote the notes, another channel's
   read cannot be undone by branching. For the reader it is an input, like a tool result, and is
   recorded as such.
5. **Provenance is recorded, exactly and structurally.**
   - Every write to a knowledge store records its origin: session, channel, triggering message and
     the context digest at the time of writing ([06](06-context-in-message-for-auditability.md)).
     Whether an entry is on the active branch is then whether its origin lies on the path to the
     head.
   - Every read records the *state of knowledge* it saw: the digest of the head of what was read,
     per channel and source. It goes where it is already recorded what an agent saw, e.g.
     `ContextRef` gains `knowledge: Vec<KnowledgeRef { channel, source: Notes | History, digest }>`.
     Knowledge is accessed through tools or through the system prompt, both code under our control,
     so the reference can be exact. The text itself is recorded anyway, in the tool result or in the
     stored system prompt, but only a structured reference can be queried.
   - Knowledge written by an agent carries the references it had read (lineage), even if nothing
     evaluates them yet.
6. **A branch switch deletes nothing.** Notes of the abandoned branch stay in the tree, so
   references to them stay resolvable. What changes is their status: reads in other channels that
   relied on them become recognisable as "based on notes discarded in channel A". First step: mark
   such answers in the UI. Later, if needed: tell the reading agent, and follow lineage
   transitively.
7. **Knowledge that outlives the session is committed, not written.** It first exists bound to a
   branch (point 2) and is taken over into a store beyond the session only at a defined point,
   explicitly ("remember", a tool that can require approval) to begin with. Until then it can be
   forgotten without consequence. The commit is itself an effect that leaves the reversible zone.
8. **"Forgetting" means retracting and invalidating.**
   - Possible: retracting entries whose origin lies in an abandoned branch or a deleted session, if
     knowledge is stored as atomic entries.
   - Possible with lineage: marking or retracting what was derived from them. Summaries mix sources
     and have to be recreated instead.
   - Not possible: rewriting other conversations that already used the knowledge (history is the
     record of what happened), at most marking them; forgetting from model weights, so knowledge is
     never learnt into weights; undoing effects outside the system.

9. **A channel offers two readable sources: its notes and its history.** Another channel may read
   not only the curated notes but also the raw history of a channel's active branch. It is the same
   mechanism: the history has been a digest chain since
   [06](06-context-in-message-for-auditability.md), and the head digest names exactly what was read.
   A whole history rarely fits a prompt ([16](16-context-compression.md)), so another channel's
   history is read through a tool with a range or a search, not through the system prompt.
10. **Documents several channels work on do not branch.** A document edited by several channels
    cannot follow the branches of any one of them: if A forks, B's changes may already build on A's
    discarded ones. By the rule in point 1 such a document belongs to the *world*: a shared
    workspace outside the branches (e.g. a document store, later perhaps a git repository), edited
    through tools, every change an effect ([20](20-transactional-effects.md)) that a branch switch
    does not undo but carries over as a fact. So there are two ways to work together, with
    deliberately different semantics: notes (branch, owned by one channel, revocable) and the shared
    workspace (no branches, everyone writes, every change an effect). The workspace is not needed
    now; it is recorded here so that notes are not bent to serve it.
11. **Reading across channels is checked.** Channels may have different participants
    ([46](46-channel-participants.md)). If an agent in B reads A's history or notes, a user who is
    only in B can learn through the agent what was said in A. Reading another channel is therefore
    an action the `SecurityManager` checks, e.g. `ReadChannel(&attributes_of_A)`. Whom it checks is
    the question of [12](12-user-and-agent-permissions.md): the agent alone makes it a bridge for
    every user in B; all participants of B ("B may read A only if everyone in B may read A") leaks
    nothing. Until [12](12-user-and-agent-permissions.md) is decided, the strict rule applies: all
    **users** of the reading channel must be allowed to read the source channel. (Corrected
    2026-09-25: first "all participants"; but under the ownership policy an agent may read a channel
    only if it takes part in it, so counting agents would block reading whenever two channels have
    different agents. The leak the rule guards against is a person learning what they may not see;
    agents act for their owner, and what an agent passes on reaches the users of its channel, who
    are checked.)

12. **An agent has private knowledge across its channels, bounded by confidentiality.** Besides its
    public notes, every agent keeps a chain of private notes per channel, in its
    `AgentChannelState` - the same mechanism as point 2, differing only in who may read it; they
    branch with the channel like the agent's history. Two chains rather than a visibility flag per
    entry, so that the digest of the public chain does not reveal that private entries exist. *Agent
    session memory* is the union of an agent's active private chains across its channels, readable
    only by the agent. Every private note carries the security attributes of the channel it was
    written in and appears in another channel only if that channel may read the origin under point
    11: the agent knows everything, but recalls it only where that tells no one anything new.
    Relying on the model's discretion instead was rejected as a leak
    ([51](51-agent-private-notes.md)).

## Terms (decided 2026-09-25)

The same table is in `ARCHITECTURE.md` (section *Terms*); "branches" means follows the branch of the
conversation it came from.

| Term | Meaning | Scope | Lifetime | Branches |
|---|---|---|---|---|
| **History** | The record of what happened, never rewritten or compressed: per channel, and per agent in a channel with what each own message was produced from. | channel / agent in channel | session | yes |
| **Context** | The messages an agent's model is given as the conversation (`MessageContext`); derived from the history, may differ from it ([16](16-context-compression.md)). | agent in channel | session | yes |
| **Prompt part** | Text given besides the conversation - the system prompt, notes - transient, recorded by digest ([47](47-prompt-parts-with-provenance.md)). | turn | transient | - |
| **Model input** | What one call of the model is given: prompt parts, context and the turn's messages; reconstructable through `ContextRef`. Still called "context" in code ([50](50-turn-context-naming.md)). | one call | transient | - |
| **Turn state** | Scratch, plan, pending tool calls, the turn's messages. | agent in channel | turn | discarded |
| **Notes** | Entries in an agent's public note chain of one channel, published there, with provenance; only the author writes and revokes ([17](17-information-sharing-within-session.md)). | agent in channel | session | yes |
| **Session memory** | The union of the active public note chains of all agents in all channels of a session; a view. | session | session | per channel |
| **Private notes** | An agent's private note chain in one channel - the same mechanism as notes, readable only by the agent, with that channel's confidentiality ([51](51-agent-private-notes.md)). | agent in channel | session | yes |
| **Agent session memory** | The union of an agent's active private note chains; a view, only for the agent, each note only where its confidentiality allows. | agent in session | session | per channel |
| **Agent knowledge** | What an agent keeps across sessions, committed from notes ([18](18-knowledge-sharing.md)). | agent | permanent | no |
| **Shared knowledge** | Knowledge shared by several agents or users ([18](18-knowledge-sharing.md)). | group / global | permanent | no |
| **Workspace** | Documents several channels edit together; outside the branches, changes are effects ([20](20-transactional-effects.md)). | session or beyond | until deleted | no |

- **State** is the container of the `StateStore`, not a tier; **entities** are what the
  `EntityStore` keeps.
- The structs named `…Context` (`SecurityContext`, `EventContext`, `ComponentContext`,
  `SessionContext`, `CommandContext`, `TurnContext`) are *operation contexts*, unrelated to the
  model's context, and are not renamed. In prose, "context" alone means the model's context.
- The four tiers of `TODO.md` map as: agent channel context → context; agent session memory → agent
  session memory (private notes); agent global knowledge → agent knowledge; global knowledge →
  shared knowledge.

## Order of work (decided 2026-09-25)

Notes first, branching afterwards: notes are additive (components on existing extension points) and
useful at once, branching reaches into the core. What makes that order safe is phase 0: history,
context and notes share one chain type, so branching later turns all of them into trees at once. The
steps are small and each can be delivered on its own.

**Phase 0 - foundation**
1. Prompt parts placed and recorded on one path, with a transient position before the turn's input
   ([47](47-prompt-parts-with-provenance.md)).
2. One chain type for history and context, a pure refactor ([48](48-shared-digest-chain.md)).
3. Terms defined here and in `ARCHITECTURE.md`: history, context, notes, session memory (this
   issue's "done when").

**Phase 1 - notes within a channel** ([17](17-information-sharing-within-session.md))

4. Model: the note chain in `ChannelState`, entries "note" or "revoke N", each with provenance; the
   current state is the chain read from start to head.
5. Writing: a tool provider with `note` and `revoke_note`.
6. Reading in the own channel: a `PromptProvider` with a `BeforeTurnInput` part, not the system
   prompt, so the prompt cache survives changing notes.
7. Web UI: a notes panel per channel, read-only, with provenance.

**Phase 2 - across channels** ([17](17-information-sharing-within-session.md))

8. `ReadChannel` with the strict rule of point 11.
9. `read_notes(channel)` and `KnowledgeRef` in `ContextRef`.
10. Session memory as a prompt part: the active chains of all channels (optional).
11. `read_history(channel, range/search)`.
12. The context panel ([41](41-show-message-context.md)) shows which state of knowledge went in.

**Phase 2b - private notes of an agent** ([51](51-agent-private-notes.md)), using the reading
mechanism, `KnowledgeRef` and access rule of phase 2; steps in that issue.

**Phase 3 - branching** ([08](08-retry-and-edit-past-answers.md))

13. The chain type becomes a tree: head, `fork_at`, `switch_to`, path.
14. Regenerate the latest answer: `ChannelLoop::regenerate`, `BranchSwitched`, turn state reset,
    `BranchChannel`.
15. REST and web UI: the regenerate button.
16. Regenerating earlier answers and switching branches (`< n/m >`).
17. The world does not branch: tools declare effects, effects of abandoned branches carried over as
    facts (part of [20](20-transactional-effects.md)).
18. Marking reads based on discarded notes (point 6).
19. Editing a past user message.

**Phase 4 - knowledge beyond the session** ([18](18-knowledge-sharing.md)), built on the commit of
point 7.

## Done when

- The tiers are defined here: scope, lifetime, reader, writer, invalidation, storage, access rules.
- The terminology is reconciled with `ARCHITECTURE.md`'s existing use of "context", "history" and
  "state".

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Principles for branching and provenance decided and recorded above, prompted by
  the branching plan in [08](08-retry-and-edit-past-answers.md): a third axis (bound to a branch or
  not), notes that branch with their channel and are read directly by other channels, reading across
  channels as an effect, exact structured provenance of writes and reads, commit instead of write
  for knowledge beyond the session, and what "forgetting" can and cannot mean. Refined the same day:
  session memory is the union of the channels' active note chains, each entry owned by one channel;
  revocations and new versions are part of the chain (3a); a channel offers its notes and its
  history (9); documents several channels edit are a shared workspace outside the branches (10);
  reading across channels is checked, strictly until [12](12-user-and-agent-permissions.md) decides
  (11).
- **2026-09-25** — Order of work recorded above: phase 0 (new issues
  [47](47-prompt-parts-with-provenance.md) and [48](48-shared-digest-chain.md)), phases 1 and 2
  under [17](17-information-sharing-within-session.md), phase 3 under
  [08](08-retry-and-edit-past-answers.md), phase 4 under [18](18-knowledge-sharing.md). Notes are
  read as a transient prompt part before the turn's input, not through the system prompt, for the
  prompt cache ([47](47-prompt-parts-with-provenance.md)).
- **2026-09-25** — Terms defined above and in `ARCHITECTURE.md` (step 3 of phase 0). "Agent session
  memory" is kept, as private notes of an agent per channel that carry the confidentiality of their
  origin (principle 12, [51](51-agent-private-notes.md), phase 2b). Renaming "context" for the model
  input in `TurnContext` was deferred to [50](50-turn-context-naming.md).
- **2026-09-25** — Corrected before implementing phase 1: notes belong to the **agent** that wrote
  them, not to the channel; the channel is where they are published (branch and confidentiality).
  Principles 2, 3, 3a and 12 and the terms were changed accordingly: public and private notes are
  two chains of one mechanism per agent and channel in `AgentChannelState`, the agent is the only
  writer and taken over when its turn completes, only the author revokes, and the notes of a channel
  and session memory are views over the agents' public chains.
- **2026-09-25** — Point 11 corrected while implementing phase 2: the strict rule checks the
  **users** of the reading channel, not all participants. With agents counted, reading would be
  blocked whenever two channels have different agents, because an agent may only read channels it
  takes part in.
- **2026-09-27** — Principles 11 and 12 changed while deciding
  [61](61-reasoning-leaves-the-agent.md): whether one channel may read another no longer depends on
  its participants but on the channels' confidentiality levels (`public < session < channel`).
  Channel A may read B if `B.level <= A.level` and A is within B's reach (`public`: always;
  `session`: same session; `channel`: only A itself); private notes are recalled by the same rule.
  The per-user check "every user of the reading channel may `ReadChannel` the source" is replaced,
  because it changed its answer whenever someone joined or left.
- **2026-09-28** — Principle 7 ("knowledge that outlives the session is committed, not written") is
  deliberately not followed by agent memory ([62](62-json-store-concurrent-writes.md),
  [68](68-notes-in-agent-state.md)): `remember` writes directly, and entries written in `public`
  channels are recalled in the agent's other sessions too and survive the deletion of the session
  they were written in. Knowledge from more confidential channels stays bound to them by the recall
  rule.
- **2026-09-28** — Terms changed ([50](50-turn-context-naming.md)): *context* is what one call of
  the model is given, assembled from prompt parts, the transcript and the turn's messages -
  "context" in the usual LLM sense. The stored sequence of an agent's messages the context is built
  from, called *context* in the table above, is now the *transcript*. The term *model input* is
  superseded by *context*.
- **2026-10-15** — Closed as done. Both points of *Done when* are met: the tiers are defined by the
  terms in [MEMORY.md](../docs/MEMORY.md#terms) - scope, lifetime and branching each, with readers,
  writers and storage described in that document's sections on notes, private notes, agent memory
  and reading other channels -, and the terminology is reconciled with `ARCHITECTURE.md`. The
  principles above are decided and built in their own issues; what remains open of the tiers is
  knowledge shared between agents ([18](18-knowledge-sharing.md)) and memory as documents
  ([166](166-memory-as-topic-documents.md)), each in its own issue.
