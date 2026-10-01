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

## Plan, detailed (2026-10-01)

Drawn up for the implementation to begin; it supersedes the draft above
where the two differ (the draft stays as the record of the first thought).
Step 1 of the draft is done: every chain is a tree with a head. What follows
is the rest, with the target picture, the components, their interfaces and
the steps. What it decides ahead is marked as a proposal; the decisions to
take are listed at the end.

### Target picture

1. **The conversation of a channel is a tree; what is shown and given to
   the models is its active path.** The channel's `ChannelState.history`
   says which path is active; every agent's chains in the channel - its
   history, its transcript, its notes and private notes - follow it. The
   given-up turns of [145](145-ended-turns-leave-a-record.md) are branches
   of an agent's transcript only; a branch of the conversation is in every
   store.
2. **A branch is named by a message id**, never by a digest: a digest is
   one store's, a message id is in every store that saw the message. The
   channel translates the id into its digests, every agent into its own.
3. **Three operations, one mechanism.** *Regenerate* an agent's answer,
   *switch* to another branch, *edit* a user's message: each is a switch of
   the channel to a point of its conversation, followed by something said
   or asked there - a `ResponseRequested` to the answer's author, nothing,
   or the user's new message.
4. **A switch is an event in the channel's order.** `ChannelLoop` moves
   its own head first and then tells everyone, `BranchSwitched { head }`,
   between the events it routes - as a removal is announced (issue 162).
   Every agent moves its heads when the event reaches it; the users' UIs
   reload. Nothing is deleted; the recorder sees the switch as any other
   event.
5. **The stores differ, and the channel's tree is the reference.** The
   moderator decides who is given which messages (and
   [174](174-moderator-as-component.md) will make that a component's
   decision); a late joiner holds the conversation from its start on
   (issue 46), its history less than its transcript. So no two stores hold
   the same messages, and what has to hold instead is: *every store's tree
   is the channel's tree projected onto the messages it holds* - a message
   the store lacks is contracted - *with the store's own internal entries
   and dead branches hanging off it*, and *every store's head is the
   projection of the channel's head*. Every operation below is defined so
   that it keeps this.
6. **The channel places, everyone follows its placement.** A message of
   an agent says which message of the conversation it was given to
   (`Message::after`: the last message of the agent's history as its turn
   began; `None` for a user's message, which follows whatever is last).
   The channel places the message at the end of the branch through that
   message - at its head when the message is on its active path, which a
   linear conversation always has - and tells everyone with the message
   where it put it: the message of the entry it was placed behind
   (`placed_after`). A store places it behind the deepest message of the
   channel's path to `placed_after` that it holds - the message itself in
   the normal case, else as the channel tells it the path. So a turn that
   is under way when the channel switches ends on the branch it began on,
   in the channel's history and in every store alike, whichever of the two
   processes the switch first, and a switch needs no moment in which
   nothing is in flight.
7. **Derived state follows the conversation** (principles in
   [19](19-memory-tiers.md)): the note chains move with the message
   chains, by the message each note was written after. The working state
   of a turn (`AgentChannelState::turn`) does not need to: a turn ends on
   its branch (point 6).
8. **The world does not branch.** A tool that ran on a branch no longer
   active has had its effect. The model is told of such calls as a prompt
   part computed from the tree (the decision below), not by a message
   written into a branch: a switch moves heads and writes nothing.
9. **Permission.** Switching changes what everyone in the channel sees;
   like cancelling it is an operation on the channel as a whole, one
   decision for the channel: `ActionType::BranchChannel`.

### The components

| Crate | What changes |
|---|---|
| `caelumora-model` | `Message::after`, covered by the digest; `FullResponseGenerated` carries `placed_after`; `Chain`: placing an entry under an anchor, the parent of an entry, whether an entry is active; `MessageHistory`/`Transcript`: placing by a path of message ids, switching by one, the forks of the active path; `Ledger`: following a path; `AgentChannelState::switch_to`; `ActorEvent::BranchSwitched`; `ActionType::BranchChannel`. |
| `caelumora-runtime` | `ChannelLoop::regenerate`, `switch_to`, the switch in `run`'s order (`ChannelChange`), `append_history` placing by `after` and stamping `placed_after`, `active_path` and `path_to`; the moderator routes `BranchSwitched` to everyone and keeps `placed_after` when forwarding; `AgentActor` resolves where a perceived message goes and handles the switch between turns (`Turn::switch_to`); `commit_turn`/`give_up_turn` set `after`; `UserActor::edit_message`; a prompt part for the calls off the active path, replacing 145's told message. |
| `caelumora-rest-server` | `POST …/messages/{mid}/regenerate`, `POST …/branch`, `POST …/messages/{mid}/edit`, `GET …/forks`; the snapshot carries the forks; `BranchSwitched` and the changed `FullResponseGenerated` go through the stream as every event. |
| `caelumora-web-ui` | A regenerate button on an agent's answer; `< n/m >` at forks; editing one's own message; a reload on `BranchSwitched`; late messages of a branch no longer shown are dropped; the new shape of `FullResponseGenerated`. |
| `caelumora-tui` | Prints the switch; the new shape of `FullResponseGenerated`. |
| docs | CHANNELS.md (*Acting on the channel as a whole: Branching*; how a store follows the channel), AGENT-TURN.md (*The transcript as a chain*, what a message follows, the calls off the path), MEMORY.md (notes follow the branch), SECURITY.md (`BranchChannel`), EVENTS.md (`placed_after`), REST-SERVER.md, WEB-UI.md, ARCHITECTURE.md (*Terms*). |

### The interfaces (sketch)

**Model.**

```rust
pub struct Message {
    …
    /// The message of the conversation this one was given to, as its
    /// sender saw the conversation (issue 08): for an agent's message the
    /// last message of its history as the turn began; `None` for a
    /// user's, which comes after whatever is last. Where the channel
    /// places the message, and where `regenerate` forks. Covered by the
    /// digest. Not where any store put it - that is the chain's
    /// (`placed_after`, below).
    pub after: Option<MessageId>,
}

pub enum ActorEvent {
    …
    /// The end of a turn, or a user's message (…as today…). `placed_after`
    /// is the message behind which the channel placed the first of
    /// `messages` - `None`: as a root - stamped by the channel as it
    /// routes the event, as it resolves the mentions; a recipient places
    /// the messages there too (issue 08).
    FullResponseGenerated { messages: Vec<Message>, placed_after: Option<MessageId> },
    /// The channel went on from another point of its conversation (issue
    /// 08): `head` is the last message of its active path now, `None` for
    /// none. Every agent moves its chains with it; a UI reloads.
    BranchSwitched { head: Option<MessageId> },
}
pub enum ActionType<'a> { …, BranchChannel(&'a SecurityAttributes) }

impl<E: ChainEntry> Chain<E> {
    /// Whether the entry is on the active path.
    pub fn is_active(&self, digest: &Digest) -> bool;
    /// The digest of the entry `digest` follows; `Some(None)` for a root.
    pub fn parent(&self, digest: &Digest) -> Option<Option<Digest>>;
    /// Appends under `anchor`: after the head, as `push_with`, if `anchor`
    /// is on the active path or `None`; else as a child of `anchor`
    /// itself, without moving the head.
    pub fn push_under(&mut self, anchor: Option<&Digest>, make: …) -> Option<Digest>;
    /// Moves the head to the deepest entry `keep` accepts, or to the root
    /// if it accepts none: how a store follows a path named by message ids.
    pub fn move_head_to_last(&mut self, keep: impl Fn(&E) -> bool);
}

/// Where a store puts a message it is given: the channel's path to the
/// message it was placed behind, root first - a store takes the deepest
/// one it holds. Just `[placed_after]` when the store holds it.
pub type Placement<'a> = &'a [MessageId];

impl MessageHistory {                       // Transcript alike
    /// Appends `msg` under the deepest message of `placement` it holds
    /// (`push_under`); at the head when it holds none.
    pub fn push_at(&mut self, placement: Placement, msg: Message, context: Option<ContextRef>);
    pub fn holds(&self, id: MessageId) -> bool;                          // on any branch
    pub fn entry_of(&self, id: MessageId) -> Option<&HistoryEntry>;      // on any branch
    /// The message before `id` on the active path; `Some(None)` for the first.
    pub fn before(&self, id: MessageId) -> Option<Option<MessageId>>;
    /// Moves the head to the deepest message of `path` it holds (the root if none).
    pub fn switch_to(&mut self, path: &[MessageId]);
    /// Where a switch to the branch through `id` goes on: its last message.
    pub fn end_of_branch(&self, id: MessageId) -> Option<MessageId>;
    /// The path from the root to `id`, as message ids.
    pub fn path_to(&self, id: MessageId) -> Option<Vec<MessageId>>;
    /// Where the active path has siblings: for the UI's `< n/m >`.
    pub fn forks(&self) -> Vec<Fork>;
}
pub struct Fork { pub after: Option<MessageId>, pub alternatives: Vec<MessageId>, pub active: usize }

impl<K: LedgerKind> Ledger<K> {
    /// Moves the head to the last entry written after a message of `path`.
    pub fn switch_to(&mut self, path: &[MessageId]);
}

impl AgentChannelState {
    /// Follows the channel to the branch whose active path is `path`:
    /// history, transcript, notes and private notes.
    pub fn switch_to(&mut self, path: &[MessageId]);
}
```

The channel's own history is the only store that places by `after`
(`end_of_branch` - the chain's `leaf_from` - when `after` is off the
active path). A recipient never follows `leaf_from`: its transcript has
dead branches, which `leaf_from` would run into; it is told the entry and
puts the message under it. What hangs off that entry already - the
internal entries of a given-up turn - becomes a sibling, which is where a
dead branch belongs.

**Runtime.**

```rust
impl ChannelLoop {
    /// Goes on from the branch through `message`, at its end, and asks
    /// nobody. Turns under way end as on a cancel, on their branch.
    pub async fn switch_to(&self, context: &SecurityContext, message: MessageId) -> Result<()>;
    /// Asks the author of `message` - an agent's, on the active path - to
    /// answer again: the channel goes on from the message it was given to
    /// (`Message::after`, else the one before it) and sends the author a
    /// `ResponseRequested`. Turns under way end as on a cancel.
    pub async fn regenerate(&self, context: &SecurityContext, message: MessageId) -> Result<()>;
    /// The message ids of the active path: what an agent follows on a switch.
    pub(crate) async fn active_path(&self) -> Vec<MessageId>;
    /// The channel's path to `message`, on whichever branch: what an agent
    /// asks when it is to place a message behind one it does not hold.
    pub(crate) async fn path_to(&self, message: MessageId) -> Option<Vec<MessageId>>;
}
/// What `run` applies between the events it routes (today `ActorChange`).
enum ChannelChange {
    Start(Arc<dyn Actor>),
    Stop(ActorId, SecurityContext),
    /// Move the head to the entry holding `to`, announce `BranchSwitched`,
    /// then ask `then` to respond, if anyone.
    Switch { to: Option<MessageId>, by: SecurityContext, then: Option<ActorId> },
}
impl UserActor {
    /// `switch_to` the message before `message`, then `send_message`.
    pub async fn edit_message(&self, message: MessageId, content: String, classification: Option<Classification>) -> Result<Uuid>;
}
impl Turn<'_> {
    /// Where the messages perceived next go (`Placement`); the perceive
    /// processors' `extend_transcript`/`extend_history` place by it.
    pub(crate) fn place_at(&mut self, placement: Vec<MessageId>);
    /// `AgentChannelState::switch_to`; the turn under way, if any, is untouched.
    pub(crate) fn switch_to(&mut self, path: &[MessageId]);
}
```

- **Placing in the channel.** `append_history` places a
  `FullResponseGenerated` by its messages' `after` (the first message's;
  the rest follow it) and stamps `placed_after` into the event before it
  is routed, where `resolve_mentions` stamps the mentions. The moderator's
  `forward_routes` keeps it; `as_user_messages` keeps `after`.
- **Placing in an agent.** `AgentActor::handle_waiting` resolves the
  placement before `perceive`: `[placed_after]` if both chains hold it,
  else the channel's `path_to(placed_after)` (through
  `ActorRuntime::channel_loop`, as `request_response` asks for the
  classification; a channel that is gone places at the head), and gives it
  to the turn with `place_at`. The perceive processors are unchanged but
  for calling `push_at`. The agent's own messages are pushed at its head
  by `record_turn`, as today: the heads do not move during a turn, so the
  head is the turn's base, and that is where the channel places them too
  (`after` is the base's last message).
- **Following a switch.** `handle_waiting` handles `BranchSwitched` like
  a perceived message: it asks its channel for `active_path()` and calls
  `turn.switch_to(&path)`; the agent's inbox filter admits the event. An
  agent in a turn reads its inbox afterwards and follows then; its turn's
  messages went where they belong (point 6).
- `commit_turn` and `give_up_turn` set `after` on the turn's messages to
  the history's last message id as the turn began (the last message at
  commit time, see above); `UserActor::send_message` leaves it `None`.
- **Regenerate:** `cancel_actors()` first (quiet, every turn under way
  ends as on a cancel), then the `Switch` through the changes queue; `run`
  moves the head, announces `BranchSwitched` through `process_envelope` in
  the caller's name, clears `quiet` and forwards a `ResponseRequested` to
  the author alone, in the regenerate's span. What the author then says
  is routed as any answer: the moderator asks whom it would ask. That is
  the multi-agent rule (draft point 11): only the author answers the fork
  message again; others react to the new answer as they would have.
- `switch_to` is the same without a `then`, and `edit_message` is a
  `switch_to` to the message before, followed by the user's message
  through its actor - so that the message is admitted, placed and routed
  as any other.
- Errors: a message not on the active path, not an agent's (regenerate),
  not the user's own (edit) or unknown is a `ChannelError` that says so,
  mapped to 404/409 as the routes' errors are today; a refusal is
  `Forbidden`.

**REST and web UI.**

| Route | Does |
|---|---|
| `POST …/messages/{mid}/regenerate` | `ChannelLoop::regenerate` |
| `POST …/branch` `{ message }` | `ChannelLoop::switch_to` |
| `POST …/messages/{mid}/edit` `{ content, classification? }` | `UserActor::edit_message` |
| `GET …/forks` | `Vec<Fork>` of the active path; also in the channel snapshot as `forks` |

The web UI: a regenerate icon in the footer of an agent's message (next to
copy and context), shown while nothing is being generated; `< i/n >` at a
message that is one of a fork's alternatives, switching with
`POST …/branch`; an edit icon on the user's own messages; on
`BranchSwitched` it reloads `GET …/messages` and `GET …/forks` and ends
any streaming it shows. A `CompleteMessageGenerated` whose `after` is not
among the messages shown belongs to a branch no longer active (a turn that
ended after the switch) and is dropped.

### Rules that hold throughout

- A switch moves heads; it never adds, removes or rewrites an entry. The
  digests of every branch stay what they were, so every `ContextRef` keeps
  resolving (`prefix` finds an entry on any branch).
- Every store's tree is the channel's projected onto what it holds (target
  picture, point 5). Placing keeps it: a message goes behind the deepest
  held message of the channel's path to where the channel put it, and at
  the head when that is on the active path - which is exactly today's
  behaviour, including the order in which a store that commits its own
  answer before perceiving another's has the two. Switching keeps it: a
  store moves to the deepest held message of the new path, the root if it
  holds none - so an agent that joined later (issue 46) sees nothing from
  before its start on any branch, as it should.
- The channel's history and every agent's history and transcript are
  moved by the same sequence of switches and placements, each applied in
  the channel's order. That is the invariant the tests of step 2 check,
  with a moderator that does not forward everything.
- `all_entries` keeps serving what has to consider every branch: finding
  a message by id, `fit_into`, the next number of a note.

### Steps

Each compiles, passes and can be committed on its own.

1. **Placement.** `Message::after` in the model and the digest;
   `FullResponseGenerated { messages, placed_after }` with the frontends
   following the shape; `Chain::push_under`, `is_active`, `parent`;
   `MessageHistory`/`Transcript::push_at`, `holds`, `path_to`,
   `end_of_branch`; `append_history` placing and stamping;
   `ChannelLoop::path_to`; the agent resolving the placement before
   `perceive`, `Turn::place_at`; `commit_turn`/`give_up_turn` set `after`,
   `send_message` leaves it `None`. No change in behaviour for a linear
   conversation. Tests: a message placed behind an entry off the active
   path lands there, in a history and in a transcript with a dead branch
   hanging off that entry; a store that does not hold `placed_after`
   places behind the deepest it holds of the path; one that holds nothing
   of it places at its head.
2. **The switch.** `BranchSwitched`, `BranchChannel`, `ChannelChange::Switch`,
   `ChannelLoop::switch_to` and `active_path`, the moderator routing the
   event to everyone, `AgentChannelState::switch_to` with the chains'
   `switch_to` and the ledgers', `Turn::switch_to`, the agent handling the
   event between turns. Tests in `channel.rs`: two agents and a user,
   with a moderator that forwards one agent only part of what is said;
   a switch moves the channel's history and both agents' histories,
   transcripts and notes to the projection of the same message; a switch
   while an agent answers leaves its answer on the old branch everywhere;
   a store that holds nothing of the path goes to its root; the given-up
   turns of 145 stay where they are.
3. **The calls off the active path.** The prompt part (decision below):
   `give_up_turn` stops writing its told message; the part lists the
   executed calls on entries off the active path, with 145's
   classification rule (in full where the call's classification may flow
   into the channel, else by name). Tests: a turn after a given-up one is
   told of its executed calls as before; switching back and forth tells
   nothing twice. Before regenerate, so that a regenerated turn never
   repeats a call the abandoned answer ran.
4. **Regenerate.** `ChannelLoop::regenerate`, the REST route, the web
   UI's button with the reload on `BranchSwitched` and the dropping of
   late messages, the TUI's line. Verified in the browser with a model.
   From here on retry works for a single-agent channel - the issue's
   "done when".
5. **Switching and forks.** `MessageHistory::forks`, `GET …/forks`, the
   forks in the snapshot, `POST …/branch`, the web UI's `< i/n >`.
   Regenerating an earlier answer works with the same code: the fork is
   wherever the answer was given to.
6. **Editing a user's message.** `UserActor::edit_message`, the route, the
   web UI's edit on one's own messages.
7. **Documentation and close.** The documents in the table; the draft
   plan's open questions answered in History; `rejected`/`done` recorded
   for the alternatives below.

### Decisions to take before step 1

- **Two fields that say "after".** `Message::after` is the sender's: what
  the message was given to, set once, the same in every store, in the
  digest - unlike the predecessor `prev_id` once was
  ([117](117-message-prev-id.md)), which was each chain's own and
  overwritten per chain. `placed_after` on the event is the channel's:
  where it put the message, which may be later than `after` (two agents
  answering the same message: the second is placed behind the first).
  Neither is the chain's predecessor, which stays `parent` on the entry.
  Proposal: both, named apart as here, and documented against each other
  in EVENTS.md and AGENT-TURN.md. Alternative: only `placed_after`, with
  `regenerate` walking the channel's path back to the turn's first
  message - which needs a turn boundary the history does not record.
- **How the model learns of calls off the active path.** 145 tells them in
  an internal system message on the active branch when a turn is given up;
  that was right while branches only grew. With switching, telling on
  every switch would write a message into whichever branch is entered,
  again and again, unless deduplicated against what is told on it already
  - a switch that changes a branch - and the message, hanging off the fork
  as an internal entry, would be left as a sibling by the placement rule
  above. Proposal: a prompt part instead, a `PromptProvider` component
  (the pattern of notes) that lists the executed calls on entries off the
  active path - with arguments and results where their classification
  allows, by name otherwise - recorded like every part by digest in the
  `ContextRef`. Then switching changes no branch, nothing is told twice,
  and `give_up_turn` stops writing the message. Cost: the model sees the
  list every turn, as it sees its notes, and 145's message and its tests
  go. Alternative: keep 145's message and tell at a switch, deduplicated
  by call ids kept in the message's metadata.
- **Who may branch.** Proposal: as cancelling and waking - whoever may read
  the channel, by the policy's grouping of `CancelChannel`, since a
  contributor who cannot retry an answer has the single most-used
  affordance missing. Editing one's own message additionally requires
  the message to be one's own; the policy may narrow either later.
- **A turn under way at a switch.** Proposal: both `regenerate` and
  `switch_to` end turns under way as a cancel does (taken over, on their
  branch), so that nobody keeps generating into a branch no longer shown.
  Point 6 makes this a matter of courtesy, not of consistency.
- **Mentions and the moderator after a regenerate.** Proposal: only the
  author is asked again; what follows its answer is routed as any answer.
  Alternative: ask everyone the moderator would ask for the fork message,
  which makes "regenerate this answer" produce several.

### Known edges

- A note written in a turn that produced no message is `after` the message
  before the turn, and stays active when the turn's branch is left. Rare;
  noted, not solved.
- Two stores may hold the same messages in a different order on one
  branch (an agent that commits before it perceives), as today. Nothing
  reads order across stores; the digests differ per store anyway.
- Context compression ([16](16-context-compression.md)) will store a
  context that is not the transcript; it is to be stored per branch, which
  `after` and the tree allow. Left to 16.
- "Points in time" of [163](163-navigable-views.md) are not needed here:
  switching works on message ids, not digests.
- Which tools have an effect ([20](20-transactional-effects.md)) is still
  open; until then every executed call is told, as 145 does.

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
- **2026-10-01** — A detailed plan added (the section *Plan, detailed*),
  for the implementation to begin: the target picture, the components,
  their interfaces, seven steps and the decisions to take first. What
  changed against the draft: a branch is applied store by store through
  *placement* - every message says which message of the conversation it
  comes after (`Message::after`), and every store places it at the end of
  that branch - so that a switch is an ordered event like a removal and
  needs no quiet moment, a turn under way ends on its own branch
  everywhere, and `regenerate` forks at what the answer came after. The
  turn-state reset of draft point 4 is not needed for that reason. The
  calls of abandoned branches are proposed as a prompt part instead of
  145's told message, since switching must not write into branches.
- **2026-10-01** — Revised after the maintainer's objection that the
  stores differ: the moderator decides who is given what, a late joiner
  holds less, and [174](174-moderator-as-component.md) will make the
  filtering a component's. The first version placed a message at a
  store's head when the store did not hold what it follows - right on a
  linear conversation, wrong once a switch lies between. Now the channel
  places and tells everyone where (`placed_after` on
  `FullResponseGenerated`), a store places behind the deepest message of
  the channel's path to it that it holds, asking the channel for the path
  only when it lacks the message, and the invariant is stated: every
  store's tree is the channel's projected onto what it holds. Recipients
  never follow `leaf_from` - their transcripts have dead branches - so a
  told message of 145 would be left as a sibling, one more reason for the
  prompt part; the step for it moved before regenerate.
