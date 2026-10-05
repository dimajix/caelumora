# 234 — A message once said can be neither changed nor taken back, and an answer not asked for again

**Status:** in progress
**Area:** Sessions & channels

## The problem

What was said in a channel stands as it was said. A user who mistyped, pasted the wrong thing or
asked the wrong question cannot correct or take back their message, and nobody can ask an agent to
answer again when its answer went wrong. The only way on is another message - "no, I meant ..." -
which leaves the mistake in the channel's history and in every agent's transcript, where it keeps
influencing every later turn.

This is the problem of [08](08-retry-and-edit-past-answers.md), taken up again without what made 08
too large: branching. 08 planned a tree of conversations that every store of a channel switches
between together. That solves more than is asked for, and most of its difficulty - stores that hold
different messages in different order, late answers of a branch no longer shown, what a switch means
for an external agent - comes from the switching, not from the wish to correct a message.

## Why it matters

Correcting one's own message and asking for another answer are among the most used affordances of
any chat, and their absence is felt at once. For an agent they are also the cheapest way out of a
derailed answer without discarding the session.

## Considerations

### The model: a message is changed where it stands, nothing forks

- **Others have seen it.** A message that is changed or taken back was delivered: users read it,
  agents may have answered it. The design does not pretend otherwise. Nothing is rewound and no
  second line of conversation exists; the message is changed *from now on*, in every store that
  holds it, and what was said in reply stays.
- **Two changes, by message id.** A message is *modified* (it reads differently from now on, at the
  place it has) or *revoked* (it is taken back). Both name the message by its `MessageId`, which
  is the same in every store that saw the message. A store that does not hold it - a late joiner
  (issue 46), an agent the moderator did not give it to - ignores the change. That is what makes
  08's invariant over differing stores unnecessary.
- **The chains stay append-only** ([197](197-one-chain-one-model-of-change.md)): a change is an
  entry of its own, and what stands is read from the entries - exactly as
  `HistoryEntry::Reclassified` does today (`MessageHistory::fold`). No entry is rewritten, no
  digest recomputed, so every `ContextRef` recorded before keeps resolving to what the agent saw
  *then*: the change lies after the prefix its digest names.
- **The transcript changes for the next turn.** An agent's transcript records the change like its
  history does; from the next turn on its model is given the message as it stands, or not at all.
- **Retry is a request to the author, not an operation of the channel.** Whoever wants another
  answer asks the agent that gave it. The agent decides - by its own transcript, which is what its
  model is given - whether it can, says so (`RetryAccepted` / `RetryRejected`, after the pattern
  of `ApprovalRequested` and its answers), and carries it out with the operations every author
  has: revoke its message, modify it, say something new at the end. How it does that is its own
  matter; the limits are those of the operations.
- **Retry only of the last message, and the agent says whether it is the last.** "Last" means:
  nothing follows it in the agent's transcript but the rest of the turn it belongs to. An answer in
  the middle of a conversation would have to be replaced in place, above what answered it - with a
  retry that may produce several messages (a tool call, its result, an answer) that is a fork
  with its lower part transplanted, which is what this issue leaves out. A user's own message can
  be modified or revoked anywhere: one message changes, nothing has to be placed.
- **The context of a retry** is what the message was produced from: the same place in the
  transcript, with everything before it as it stands *now* - the main use is "I corrected my
  question, answer again".

### What was weighed and not chosen

- **Branching (08).** Keeps every alternative reachable (`< 2/3 >`), at the cost described above.
  Left out here; the tree of `Chain<E>` built for it stays and is used by given-up turns
  ([145](145-ended-turns-leave-a-record.md)) and external agents
  ([205](205-external-agent-message-context.md)).
- **Retry as a modification of the one message.** A retried model call may answer with a tool call
  instead of text, so one message becomes several; and one id would have two contexts it was
  produced from (`recorded_context_of`). Retry is therefore: revoke, then answer anew - new
  messages with new ids, each with its own `ContextRef`.
- **The author's own stores by moving the head back**, as `give_up_turn` does. It would keep the
  retried answer visible as a given-up turn, but it changes the author's stores in another way
  than everyone else's, works only for the tail of a transcript, and is the mechanism this issue
  set out to avoid. The author applies its own revocation like every other store. What is taken
  from 145 is how the model is told of calls that ran (`told_of_executed`).
- **Dropping a revoked message from the history altogether.** The channel's history keeps it as a
  message without content, marked revoked: users are told that something was taken back, and
  whatever names the message by id keeps finding it (`Channel::history_limits`' `HistoryStart`,
  notes written `after` it, `missing_from`).

### What it is not

- **Revoking is not erasing.** The message stays in the chains of every store, in the contexts of
  the answers produced from it (readable by whoever may read the agent's context), in notes and
  memory derived from it, and with a remote agent that was given it. Removing content for good -
  a secret pasted by mistake - is another feature with other requirements (purging stored state)
  and not part of this issue. The documentation and the UI have to say so: "taken back", not
  "deleted".
- **The world is not taken back** ([20](20-transactional-effects.md)): a tool call of a revoked
  turn has had its effect, and the notes and memory the turn wrote stay written.

## Plan

### The events

Five new `ActorEvent`s, named as the existing ones are - what happened, or what is asked:

```rust
pub enum ActorEvent {
    …
    /// The message reads like this from now on (issue 234): same id, same
    /// sender and role as the message it changes; its `timestamp` is when
    /// it was changed. Sent by the message's author only.
    MessageModified(Message),
    /// The message `message` is taken back by its author (issue 234).
    MessageRevoked { message: MessageId },
    /// `author`, an agent, is asked to answer again instead of its message
    /// `message` (issue 234). It answers with `RetryAccepted` or
    /// `RetryRejected`.
    RetryRequested { message: MessageId, author: ActorId },
    /// The agent will answer again instead of `message`: what it revokes,
    /// modifies and says follows, and ends like every turn.
    RetryAccepted { message: MessageId },
    /// The agent will not: `reason` is meant for users, like
    /// `TurnFailed::message`.
    RetryRejected { message: MessageId, reason: String },
}
```

- `MessageModified` carries a whole `Message` so that everything that looks at the messages of an
  event covers it without a second code path: `ActorEvent::messages()` (and with it the channel's
  `admits` - the `Publish` question for the new classification - and
  `OriginEventSender::speaks_as`), `published()`, `resolve_mentions`.
- `MessageModified` and `MessageRevoked` are written to the history; the three retry events are
  not, like `TurnFailed` and `ChainCutOff`: they are shown and gone on reload.
- No `GlobalEvent`: a change reaches the users through their actors like everything said.

### The model (`caelumora-model`)

- **A change to a message, in one place.** History and transcript are two chains with two entry
  types (`HistoryEntry::Said`, `TranscriptEntry::Message`); both need the two changes and the rule
  that reads them. Proposal: one type `MessageChange { Modified(Message), Revoked { message:
  MessageId, at: DateTime<Utc> } }` that knows how to apply itself to messages as they stand, and
  one variant in each entry enum that holds it (`HistoryEntry::Changed`,
  `TranscriptEntry::Changed`). Its responsibility is its own - "a change to a message said" - and
  it is used by both chains, the digests and the state; two pairs of variants with two copies of
  the rule would be the repetition AGENTS.md warns of. To be confirmed as step 2 begins.
- **Reading what stands.**
  - `MessageHistory::fold` applies a change like a reclassification: over every branch, in the
    order things happened. A modified message keeps its place and its original `timestamp` and
    gets `Message::modified = Some(when)`; a revoked one stays as a message without content,
    reasoning, tool calls or results, with `Message::revoked = Some(when)`. Both fields are new,
    optional, not covered by the digest (like timestamps) and set by reading only.
    `Message::is_heard` is false for a revoked message by its own rule (no text), so catching up
    (`missing_from`, `perceive`) gives an agent nothing of it.
  - `Transcript::messages` applies the changes *of the active path*: a modified message in its new
    version at its place, a revoked one not at all. The same rule over a prefix is what the
    `ContextResolver` uses (`assemble`), so a `ContextRef` taken before a change resolves to the
    message as it was, and one taken after to what stands. Prompt parts placed after the entry of a
    revoked message stay where they are: the entry is still there.
  - The scopes differ on purpose: the history says how the channel's messages stand, on whatever
    branch; a transcript's digest names what one model call was given.
- **Recording.** `MessageHistory::modify(message)` / `revoke(id, at)` and the same on `Transcript`,
  each `false` and recording nothing if the chain does not hold the message (for a transcript: not
  on its active path) or it is revoked already. `AgentChannelState::change_message(change,
  perceived)` applies one change to the agent's history and transcript (see *How an agent
  reacts*).
- **Digests.** The new entries get schemes of their own in `crate::digest`, behind their own tag,
  so that every digest recorded before names what it named. A stored chain without such entries
  reads as before; nothing is migrated.
- **Permissions.** `ActionType::ModifyMessage { author }`, `ActionType::RevokeMessage { author,
  channel }` and `ActionType::RetryMessage { agent, channel }` (attributes), with
  `AccessQuestions` only if a view needs the pure question (`can_retry`, below). Ownership policy,
  proposed: modifying and revoking only by the author (`author == context.caller_id`, as
  `Publish` asks `actor == caller`); retry as cancelling and waking - whoever may read the
  channel. A turn that follows a retry acts with the authority of whoever asked for it, as a turn
  acts with its request's.

### The channel (`caelumora-runtime`, `channel.rs`)

- **Admission.** `process_envelope` asks, for a `MessageModified` or `MessageRevoked`, whether the
  message is on record, not revoked, and its author's to change: `ModifyMessage` / `RevokeMessage`
  with the author the *channel's history* names, in the envelope's security context. A modified
  message must keep sender and role. Anything else is dropped and logged, like a message the
  channel does not admit. This is the gate for every actor; what a user's actor asked before
  (below) is so that the user learns of a refusal.
- **Its history.** `append_history` records the change (`modify` / `revoke`) and stores the state,
  as for a `FullResponseGenerated`. A revocation of a message already revoked is admitted and
  changes nothing, so that it can be sent again (see *Things to watch*).
- **Mentions.** `resolve_mentions` resolves the mentions of a modified message, for display. A
  modification asks nobody to respond.
- **Waking and quiet.** A `RetryRequested` is a deliberate request to one agent: the channel wakes
  its `author` before delivering it, as it wakes whom a `ResponseRequested` asks; and the user's
  actor sends a `WakeupRequested` ahead of it, as ahead of a message, so that it is not lost in a
  channel quiet after a cancel.

### Routing (`Moderator`, `policy_moderator`)

Every moderator routes alike, through the shared `ModeratorActors`:

| Event | Goes to |
|---|---|
| `MessageModified`, `MessageRevoked` | every user, and every agent but the sender - each agent applies it if it holds the message |
| `RetryRequested` | its `author`, if that is an agent of the channel |
| `RetryAccepted`, `RetryRejected` | the users (`route_to_users`) |

Nothing goes back to the sender: an agent applies its own change itself, a user's actor shows what
its user did to those who follow it (`UserActor::send`), as for a message.

### How an agent reacts (`actor_impl`)

- **Taking a change in** is part of `actor_impl::take_in`, shared by both kinds of agent, between
  turns and in order with what is heard: an agent in a turn applies it afterwards, as it hears
  messages afterwards - the transcript does not change during a turn. The agent's `event_filter`
  admits the three new events addressed to it.
  - `MessageRevoked`: recorded in the history if it holds the message, in the transcript if the
    message is on its active path.
  - `MessageModified`: the history takes the message as it was said. For the transcript the new
    version is heard like a new message - `Message::as_heard`, then the `PerceiveInProcessor`s -
    and what passes replaces the old one; if nothing passes, the message is revoked in the
    transcript. A message that never reached the transcript does not enter it now: there is no
    place to put it.
  - No turn begins for a change, and the policy is asked nothing.
  - The state is stored and `announce_channel_state_changed` called as for a message heard.
- **A request to retry** is the second kind of request `take_in` returns. It is judged once
  everything queued is taken in - against the conversation as it stands, as a request to respond
  is answered. If a later `ResponseRequested` is in the same batch, the retry is rejected and the
  request answered.
  - The agent **accepts** if `message` is its own, an assistant message on the active path of its
    transcript, and it and everything after it belong to one turn - its last. (A tool message is
    not retried: revoking it would leave its call without a result.) How "one turn" is told from
    the transcript is to be settled in step 5: from the `ContextRef`s of its messages
    (`turn_entries` names where their turn began) or by recording the boundary.
  - It then sends `RetryAccepted`; revokes `message` and what its turn produced after it - in its
    own history and transcript, and with a `MessageRevoked` for each that is not internal; if
    calls among them ran, tells its model on the transcript, as a given-up turn is told
    (`told_of_executed`, with a reason of its own, and 145's rule for a channel lowered since);
    and begins a turn in the contexts of the request (`request_response`), as for a
    `ResponseRequested`. Its context is the transcript as it stands, which ends where the message
    was produced. The turn ends as every turn does (`FullResponseGenerated` or `TurnFailed`).
    - Retrying the final answer keeps the tool calls and results before it: no tool runs twice and
      nothing has to be told.
    - Retrying a message with tool calls revokes their results and what followed; those calls ran
      and are told.
  - It **rejects** with `RetryRejected` and a reason for users: not its message, not its last, a
    tool message.
- **An external agent** records a change in its history. Its transcript's active path holds only
  what the remote agent has not been given yet (issue 205), so a change to a message already given
  cannot be applied there, and the remote agent keeps what it was given. It rejects every retry
  ("runs elsewhere and cannot take back what it was given"). Whether changes are told to the
  remote agent is an open question.

### What a user can do (`UserActor`)

A frontend talks to the user's actor, which builds every envelope itself and sends it through
`run`'s transport - as for a message:

```rust
impl UserActor {
    /// Changes this user's message `message` to read `content`, classified
    /// `classification` - as it is classified now if `None`.
    pub async fn modify_message(&self, message: MessageId, content: String, classification: Option<Classification>) -> Result<Uuid>;
    /// Takes this user's message `message` back.
    pub async fn revoke_message(&self, message: MessageId) -> Result<Uuid>;
    /// Asks the agent that said `message` to answer again.
    pub async fn request_retry(&self, message: MessageId) -> Result<Uuid>;
}
```

Each looks the message up in the channel's history and asks before it sends, so that the user
learns of a refusal at once (the channel asks again as it admits, as for `Publish` in
`send_message`): unknown or revoked message, not the user's own (`ModifyMessage` /
`RevokeMessage`), a classification that may not flow into the channel (`Publish`), for a retry a
message whose author is no agent of the channel now, or `RetryMessage` refused. Each returns the
`span_id` like `send_message`. New `ChannelError`s name what was refused, without content.

A retry answers asynchronously: `request_retry` succeeds when the request is on its way; whether
the agent accepts arrives as an event.

### REST (`caelumora-rest-server`)

| Route | Does | Answers |
|---|---|---|
| `PUT /sessions/{id}/channels/{cid}/messages/{mid}` `{ content, classification? }` | `UserActor::modify_message` | `200` |
| `DELETE /sessions/{id}/channels/{cid}/messages/{mid}` | `UserActor::revoke_message` | `200` |
| `POST /sessions/{id}/channels/{cid}/messages/{mid}/retry` | `UserActor::request_retry` | `200`; accepted or rejected arrives on the channel's stream |

- Handlers return `Result<_, ApiError>` and reach the actor as `send_message` does
  (`open_channel`, `user_actor_of`). Errors are mapped in `error.rs`, each with its own code: `404
  message_not_found`, `403` for a refusal, `409 message_revoked`, `409 not_an_agents_message`.
- `GET …/messages`, the snapshot and an agent's history (`…/actors/{aid}/history`) return the
  messages as they stand; `Message` gains `modified` and `revoked` (absent when not set), and a
  revoked message comes without content. Additive, so the web UI of before keeps working.
- The new `ActorEvent`s go through the channel's stream like every event.
- The channel response gains `can_retry` (the `RetryMessage` question for the viewer, decided in
  the view like `can_change`), so that the UI offers retry only where it is permitted. Modifying
  and revoking are offered on the viewer's own messages; the server refuses otherwise.
- The context of a message (`…/messages/{mid}/context`) is unchanged: it shows what the agent saw
  then, also messages modified or revoked since.

### Web UI and TUI

- **Types** (`types/index.ts`): the five events, `Message.modified` / `revoked`, `can_retry`.
- **`MessageView`** (its footer takes `actions` already):
  - on the user's own messages *Edit* - the content becomes an editor with save and cancel, the
    classification picker as when sending - and *Take back*, with a confirmation that says it is
    not a deletion;
  - on an agent's message *Retry*, where `can_retry`, the message is the last of the chat and
    nothing is being generated. The agent has the last word: the button is an offer, not a
    promise;
  - a modified message is marked "edited" with the time; a revoked one is shown as a placeholder
    naming who took it back.
- **`ChatView`**: `MessageModified` replaces the message by id at its place, `MessageRevoked`
  turns it into the placeholder (and drops a partial answer of that id); `RetryAccepted` shows the
  agent as about to answer until `ResponseStarted`; `RetryRejected` shows a notice with the
  reason, gone on reload like "Chain cut off". Messages loaded later (snapshot, older pages) come
  as they stand.
- **History and context panels**: the history panel reloads on `AgentChannelStateChanged` as today
  and so shows what stands; the context panel is unchanged.
- **TUI** (`caelumora-tui`): prints a change and a retry's answer as lines. Offering the three
  actions there - e.g. a user command `/retry` for the last answer (issue 28 names retry as a
  future command) - is optional and decided in step 6.
- The client crates (`caelumora-client-api`, `-rest-client`, `-local-client`) gain the three
  operations where they offer sending.

### Things to watch

- **Nothing changes an entry** (197), and **digests are stored, never recomputed**: new entry
  variants, new digest schemes, old digests untouched.
- **Changes apply within a prefix only** when a context is resolved, or "what the agent saw" starts
  to lie.
- **Tool calls and their results stay paired** in what a model is given: never revoke one without
  the other.
- **Every reader of a transcript's messages goes through one function.** Today several read
  `entry.message()` directly (`Transcript::messages`, `ContextResolver::assemble`,
  `AgentChannelState::begin_turn`, `given_up_turns`, the external agent's label, `in_transcript`);
  each has to be looked at: which want what stands, which every entry.
- **A turn's label** (`Work::see_message`) is the join of what it has seen: `begin_turn` must see
  the messages as they stand, so that a revoked message no longer raises the label of later turns.
- **The perceive processors** see a modified message again; one that changed the old version
  changes the new one.
- **The prompt cache** of a model is lost from the changed message on. Unavoidable, and worth a
  sentence in the documentation.
- **`speaks_as`** (issue 228): a modified message must name its author as sender, which the
  `OriginEventSender` enforces through `ActorEvent::messages()` - do not route around it.
- **Lost events** ([223](223-full-inbox-drops-events.md)): an actor whose inbox is full loses a
  change, and its store then differs from the channel's for good - catching up
  (`MessageHistory::missing_from`) finds missing messages, not missed changes. Either the changes
  become part of catching up, or this waits for 223; see the open questions.
- **A crash between storing and sending**: an agent that stored its own revocations and stopped
  before sending them leaves the channel behind. Revocations are idempotent in the channel for
  that reason; sending them again on resume belongs to the retry's turn state
  ([20](20-transactional-effects.md)).
- **Logging and errors**: message content only at `trace!`; error messages name the message by
  id, never by content.
- **The security suite**: modifying and revoking another's message, a change sent by an agent for
  a message not its own, and a retry by someone the policy refuses each get their test
  (`security_suite.rs`).
- **`reclassify_message` already differs**: it changes the channel's history and announces a
  `GlobalEvent`, and no agent's history learns of it. It is the same kind of change and could take
  the path built here. Not part of this issue; named so that it is decided, not forgotten.

### Open questions

1. **May anyone but the author revoke** - the channel's owner, as moderation, or an agent's owner
   for its agent's message? Proposed: the author only at first. For an agent's message it would
   have to be a request to the agent, like retry, since only an actor speaks in its own name.
2. **Is a modified message marked for the model** when the agent answered the old version
   already? Proposed: no - the point of correcting is that the old version stops acting; the
   agent's earlier answer may then read unmotivated in its transcript.
3. **Does a modification ask anyone to respond**, or new mentions in it? Proposed: no; whoever
   wants an answer to the corrected message asks for a retry or writes on.
4. **What may an agent modify of its own message** - only content and reasoning, or tool calls
   too? Nothing in this plan makes an agent modify (retry revokes); the channel should admit
   content and reasoning only until there is a use.
5. **A retry of an answer under way.** An agent reads its inbox between turns, so a request waits
   for the turn's end. To retry an answer still being generated, its turn has to be cancelled
   first - by whom? `cancel` is channel-wide today; the channel cancelling the author before it
   delivers a retry would cancel a turn the request may not even be about. And is the id of a
   streamed partial message that of the complete one?
6. **External agents:** are changes told to the remote agent with its next call, as text? Can a
   retry be offered where the remote protocol allows it?
7. **Missed changes** (see *Lost events*): reconcile on catching up, or leave to 223?
8. **`RetryRejected::reason`**: free text like `TurnFailed::message`, or an enum the UI words?
9. **The original of a revoked message**: is it shown to anyone on purpose - an audit view, the
   recorder ([05](05-postgres-event-recorder.md)) - beyond the contexts that hold it anyway?
10. **Retry of an older answer**: stays out. Either part of a future branching issue, or built
    here later as "revoke everything below, then retry".

### Steps

Each compiles, passes its tests and can be committed on its own; the documents on the subject are
updated in the step that changes what they describe.

1. **Restructure: a transcript's messages are read in one place.** No change of behaviour: the
   places that read `entry.message()` from a transcript go through one function over a path (the
   active path, or the prefix a digest names), used by `Transcript::messages`, the
   `ContextResolver` and `begin_turn`. Tests as they are.
2. **The model.** `MessageChange`, the entries in both chains with their digests, reading what
   stands (`fold`, the function of step 1), `Message::modified` / `revoked`, `modify` / `revoke`
   on both chains, `AgentChannelState::change_message`. Tests: a modified message keeps its place;
   a revoked one is gone from the transcript and a placeholder in the history; a `ContextRef` from
   before a change resolves to the old version, one from after to the new; a change to a message
   not held records nothing; a change reaches a stored chain as a change (`Journaled`); a chain
   stored before reads as before. AGENT-TURN.md (*The transcript as a chain*).
3. **Changing and taking back, in the runtime.** The two events, `ModifyMessage` /
   `RevokeMessage`, the channel's admission and history, the routing, `take_in` applying changes
   for both kinds of agent, `UserActor::modify_message` / `revoke_message`. Tests in `channel.rs`
   with two agents and a moderator that gives one of them only part of what is said: the change
   reaches every store that holds the message and no other; an agent in a turn applies it
   afterwards and its next context shows what stands; a modified message passes the perceive
   processors; a change by anyone but the author is dropped. The security suite. CHANNELS.md,
   SECURITY.md, CONFIDENTIALITY.md (revoking is not erasing).
4. **Changing and taking back, for users.** The two REST routes with their errors, the messages
   as they stand in every response, the web UI's edit and take back, the TUI's lines, the client
   crates. Verified in the browser. REST-SERVER.md, WEB-UI.md. *From here on a user can correct
   and take back their messages.*
5. **Retry, in the runtime.** The three events, `RetryMessage`, `UserActor::request_retry`, the
   routing and waking, the agent's decision, its revocations, what it tells its model of calls
   that ran, and the turn that follows; the external agent's rejection. Tests: the final answer
   retried keeps the tool results before it and runs no tool again; a message with a tool call
   retried is told of the call; a retry that produces several messages; a retry rejected because
   something was heard since, or a later request waits; a retry twice in a row; the security
   suite. AGENT-TURN.md, CHANNELS.md.
6. **Retry, for users.** The route, `can_retry`, the web UI's button with what it shows on
   `RetryAccepted` and `RetryRejected`, the TUI. Verified in the browser with a model.
   REST-SERVER.md, WEB-UI.md. *From here on retry works.*
7. **Close.** The open questions answered in History or moved to issues of their own (5, 6, 7, 9
   and 10 are candidates), ARCHITECTURE.md (*Terms*), status `done`.

## Done when

A user can modify and take back their own messages anywhere in a channel, and ask an agent to answer
again instead of its last message; every store that holds a changed message reads it as it stands,
every agent's next turn is given it so, and what an agent saw for an earlier answer can still be
told as it was. The agent decides whether it retries and says so. Nothing forks. The rule that
reads what stands exists once, the permissions are in the security suite, and the documents
describe the events, the routes and what revoking does not do.

## History

- **2026-10-05** — Opened in place of [08](08-retry-and-edit-past-answers.md), which is closed as
  rejected: the same problem, without branching. Decided with the maintainer: a message is changed
  where it stands, by two events that name it by id (`MessageModified`, `MessageRevoked`); nothing
  forks; retry is a request to the agent (`RetryRequested`), which decides by its own transcript
  whether the message is its last and answers `RetryAccepted` or `RetryRejected`; a retry is
  carried out with the operations every author has, and for now only for the last message; a user's
  own message may be modified or revoked anywhere. The plan above was written against the code as
  of `e07aa3d`.
- **2026-10-05** — Step 1 done: a transcript's messages are read by one rule,
  `transcript::as_they_stand` over a path - every entry with the message it holds as it stands at
  the end of the path. `Transcript::messages`, the `ContextResolver`, `begin_turn` and
  `given_up_turns` read through it; `TranscriptView::usage`, `AgentContextView::in_transcript` and
  the label of an external agent keep reading every entry, since they are about what was ever
  added. No change of behaviour.
