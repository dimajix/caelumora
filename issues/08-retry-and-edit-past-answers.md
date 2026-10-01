# 08 — "Retry" and changing past answers

**Status:** in progress
**Area:** Sessions & channels

## The problem

A conversation is append-only. Once an agent has answered, there is no way to
ask for a different answer to the same question, and no way to correct a
message already sent and have the conversation continue from the corrected
version.

The workaround — say "no, do it differently" in a new message — leaves the bad
turn in the history, where it keeps influencing every subsequent turn.

## Why it matters

This is the single most-used affordance in comparable chat systems, and its
absence is felt immediately. It is also the cheapest way to recover from a
wrong tool call or a derailed plan without discarding the whole session.

## Considerations

- The hard part is not the UI, it is the state. `MessageHistory` is shared
  input to the agent's cognitive cycle; truncating it means deciding what
  happens to everything derived from the truncated turns — context, tool
  results, and any effect the tools had on the outside world. A retried turn
  cannot un-send an email. See [20](20-transactional-effects.md).
- Two designs, materially different:
  - **Truncate and re-run** — history is rewound to the edited message. Simple
    mental model, destroys the record of what happened.
  - **Branch** — the old turns stay, a new branch continues from the edit
    point. Preserves history (and suits
    [05](05-postgres-event-recorder.md)), but every consumer of history now
    has to know which branch it is on, including the moderator and every
    context provider.
- In a multi-agent channel the question compounds: retrying one agent's turn
  invalidates what the *other* participants said in response to it.
- Whichever is chosen, the `Recorder` stream should keep the discarded turns
  even if history does not.

## Plan: branching (draft, 2026-09-25)

A first rough plan for the branching design, for review. It starts with a
"regenerate" button on an agent's answer. Editing a past user message is the
same mechanism with a new message at the fork point instead of a new answer,
and comes later.

### What exists and helps

- Every `Message` has a `prev_id`, and since [06](06-context-in-message-for-auditability.md)
  every entry of a `MessageHistory` and `MessageContext` stores a digest that
  covers its predecessor's digest. The data is already a chain; a tree is
  "several entries with the same predecessor".
- Stored digests stay valid in a tree: an entry's digest depends only on its
  own path. The `ContextRef`s of old answers keep pointing at what the agent
  saw, even after a branch was abandoned.
- Forwarded messages keep their `MessageId` (`forward_routes` only changes the
  role), so one message can be found by id in the channel's history and in
  every agent's history and context.

### What stands in the way

- **Four linear stores per agent and channel.** `ChannelState.history`,
  and in every `AgentChannelState` the `history`, the `context` (which
  compression, [16](16-context-compression.md), will make differ from the
  history), and the derived state of a turn: `tool_calls` and `plan`. All are
  `Vec`s that are appended to.
- **`prev_id` is per store.** `MessageHistory::append` and
  `MessageContext::append` overwrite `prev_id` with the store's own last
  message, so the same message has different predecessors in the channel's
  history and in an agent's. A fork point therefore has to be named by the
  message id, not by a predecessor.
- **Branch state is channel-wide, stores are per actor.** A fork must move
  the channel and every agent in it at once, or they talk past each other.

### Model

1. **`MessageHistory` and `MessageContext` become trees with a head.** Entries
   keep their digest and additionally name their parent entry (by digest).
   There is one *head*, the entry the active branch ends in. `append` appends
   to the head; `messages()` returns the path from the root to the head, so
   every reader that only wants "the conversation" (the agent loop, context
   providers, REST) is unchanged. No migration of stored data is needed
   (legacy data does not have to be supported).
2. **Forking and switching.** `fork_at(message_id)` moves the head to the
   entry holding that message; the old continuation stays as a sibling
   branch. `switch_to(head)` moves the head to the leaf of another branch.
   `prefix(digest)` follows parent links instead of slicing.
3. **Branches are named by message ids.** A branch is identified by its head
   message id, which exists in every store that saw the message. Switching
   means "every store: move your head to the entry holding message X".
4. **Derived turn state.** `tool_calls` and `plan` of an `AgentChannelState`
   belong to the branch they were created on. The simplest rule: a fork or
   switch discards pending tool calls and resets the plan. Keeping them per
   branch is possible but probably not worth it at first.
   Notes ([17](17-information-sharing-within-session.md)), once they exist,
   are not turn state: they branch with the channel like its history
   (principles in [19](19-memory-tiers.md)).

### Runtime

5. **The channel forks, not the actors individually.** A new operation
   `ChannelLoop::regenerate(context, message_id)`, in line with `cancel` and
   `wakeup`:
   - Only while the channel is idle; a running turn must be cancelled first.
   - Finds the predecessor of the message in the channel history, the fork
     point.
   - Moves the channel's head there, and tells every actor to do the same:
     a new actor-level operation or event, e.g. `ActorEvent::BranchSwitched
     { head: MessageId }`, handled by every `AgentActor` on its history,
     context and turn state.
   - Sends `ResponseRequested` to the author of the regenerated message
     only.
   - Announces the switch to the UIs (the same event, routed to users).
6. **Permission.** Regenerating changes what everyone in the channel sees, so
   it is a channel-wide action like cancelling: a new
   `ActionType::BranchChannel` checked by the `SecurityManager`.
7. **Recording.** The `Recorder` stream ([05](05-postgres-event-recorder.md))
   stays append-only; the switch is an event in it. Nothing is deleted.
8. **Side effects.** Tool calls of the abandoned branch are not undone
   ([20](20-transactional-effects.md)). The UI should say so when the
   regenerated turn made tool calls.
   - **The world does not branch.** Tool calls with an effect from an
     abandoned branch are carried into the new branch as facts, e.g. a note
     "executed in a discarded branch: …, result: …", so that the new branch
     does not repeat them unknowingly. This needs tools to declare whether
     they have an effect or only read, which
     [20](20-transactional-effects.md) needs anyway.
   - **Knowledge read by others.** A switch deletes nothing, but reads of
     this channel's notes by other channels become "based on discarded
     notes" and are marked (principles in [19](19-memory-tiers.md)).

### REST and web UI

9. `POST /sessions/{id}/channels/{cid}/messages/{mid}/regenerate`, and later
   `POST .../branch` with `{ head }` to switch. `GET .../messages` and
   snapshots return the active path; each message whose predecessor has
   several continuations carries its position among them (e.g. `2 of 3`).
10. A "regenerate" icon in the footer of an agent's message (next to copy and
    context, [43](43-message-time-and-statistics.md)). Messages at a fork
    show `< 2/3 >` to switch branches. On a branch switch the chat reloads
    its snapshot.

### Multi-agent

11. Forking at the predecessor of agent X's answer drops everything after it
    from the new branch, including answers of other agents to the same
    message (e.g. under `broadcast`). Only X is asked to answer again; the
    others can be asked with a mention or `/wakeup`. Alternative: re-ask
    everyone the moderator would ask. That needs the moderator to decide
    again, and makes "regenerate this answer" produce several.

### Steps

1. Model: `MessageHistory`/`MessageContext` as trees with a head, `fork_at`,
   `switch_to`, parent-following `prefix`; tests incl. digests on branches.
2. Runtime: `ChannelLoop::regenerate`, the branch event for actors, turn
   state reset, permission, recorder event. Only the latest answer at first,
   where nothing follows the regenerated message.
3. REST and web UI: regenerate button; the chat reloads on the event.
4. Regenerating earlier answers (multi-agent rule from 11) and switching
   between branches with `< n/m >`.
5. Editing a past user message as a fork with a new message.

### Open questions

- Is the multi-agent rule in 11 the right one?
- Should turn state (pending tool calls, plan) be kept per branch after all?
- May a contributor regenerate, or only the owner?
- How does context compression ([16](16-context-compression.md)) store its
  result per branch, when it replaces older context entries by a summary?
- Which tools have an effect (point 8), and who declares it: the tool, its
  configuration, or both?

## Done when

- One of the two designs is chosen, with the reasoning recorded here.
- Retry works for a single-agent channel; the multi-agent case is at least
  explicitly deferred with a stated reason.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Requested again as a "regenerate" button, explicitly as
  branching across all actors of a channel, and filed here rather than as a
  new issue since this is the same issue. Direction: **branch**, not
  truncate. A rough plan for review was added above ("Plan: branching").
- **2026-09-25** — Plan extended with what branching means for knowledge
  and effects (points 4 and 8), decided together with
  [17](17-information-sharing-within-session.md),
  [18](18-knowledge-sharing.md) and [20](20-transactional-effects.md); the
  general principles are recorded in [19](19-memory-tiers.md).
- **2026-09-25** — Sequenced: branching is phase 3 of the order of work in
  [19](19-memory-tiers.md), after notes ([17](17-information-sharing-within-session.md)).
  Step 1 of the plan above ("trees with a head") is done on the chain type of
  [48](48-shared-digest-chain.md), so history, context and notes become
  trees at once.
- **2026-09-29** — `Message::prev_id` is to be removed
  ([117](117-message-prev-id.md)); the parent pointer of a tree of chain
  entries is to be the predecessor's digest (`prev_digest`) on the entry,
  not a field of the message.
- **2026-09-30** — [117](117-message-prev-id.md) is done: `Message` has no
  predecessor field anymore. The notes above on `prev_id` describe the state
  before; the parent pointer of a branch is `prev_digest` on the entry.
- **2026-09-30** — Turns that are given up - failed, stopped, cancelled on
  a lowering of the channel - are to become dead branches of the agent's
  transcript ([145](145-ended-turns-leave-a-record.md)), which needs step 1
  of the plan above (trees) before regenerating does.
- **2026-09-30** — Step 1 of the plan is done, for all chains at once:
  `Chain<E>` is a tree with a head. Every entry is stored with the digest
  of the one it follows (`parent`, beside the entry, not a field of the
  entry types) and the child last on the active path (`active_child`),
  and the chain with its head; the former format, a plain list, is not
  read any more. `entries`, `digest`, `last` and `len` are the active path;
  `all_entries` every branch - used for looking up a message by id
  (`recorded_context_of`, `reclassify_message`, `MessageHistory::reclassify`),
  for `fit_into`, and for the next number of a note or memory, which is
  never given twice on any branch. `prefix` finds an entry on any branch.
  Instead of `fork_at` and `switch_to`, the chain has one `move_head`;
  `children` and `leaf_from` (following `active_child`, as Open WebUI and
  Claude do when switching back to a branch) are what the `< n/m >`
  switch of step 4 needs. The wrappers get their branching methods with
  their first caller ([145](145-ended-turns-leave-a-record.md), step 2).
- **2026-10-01** — From [163](163-navigable-views.md): the views reach a
  channel's history and an agent's context as they are now; a state at a
  point in time - "as the agent saw them when it produced message Z",
  `AgentContextView::at(digest)`, `ChannelView::history_at(digest)` over
  the chains' `prefix` - was left out until a caller needs it. Branching
  is the likely one.
