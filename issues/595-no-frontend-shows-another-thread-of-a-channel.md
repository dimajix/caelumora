# 595 — No frontend shows another thread of a channel

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The web UI and the terminal show a channel's line alone (issue 557): what a sub-agent says and does in its thread cannot be followed, nobody can write into it, wake it or stop it alone, and since neither a snapshot's participants nor the items of a history page say their thread, a frontend cannot even tell what of the line belongs to another thread.

## The problem

Step 6 of 2b of issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md) made the web UI
and the terminal show a channel's line: what happens in it and in the whole channel, and of
another thread its approvals, its questions and what mentions the viewer. A view of a thread was
left for later. Missing with it:

- **Following a thread.** What a sub-agent ([558](558-no-agent-hands-a-task-to-a-sub-agent.md))
  does - its messages, its calls, its plan - is read only over REST (`?thread=`).
- **Writing into a thread** - an open point of 558: whether users may write in a sub-agent's thread
  or it is a closed circle.
- **Waking or stopping one thread.** Cancelling stops every thread of a channel, and a thread
  opened after it starts awake (557, step 4); the frontends cannot cancel or wake one thread.
- **The thread of what is shown.** A participant in a snapshot carries no thread, so a sub-agent in
  a turn as a client connects is shown as answering in the line until its turn ends; the items of
  a history page carry no thread, so the terminal cannot tell a message of another thread that
  mentions the user from one of the line - and may take it as the answer `/retry` asks for again.
- **Calls in a message shown from another thread**: their results, which mention nobody, are left
  out, so such a call may stay shown as running.

## Considerations

- **Where a thread is shown**: beside the line, under the call that opened it, or as a channel of
  its own in the list.
- **The thread on what is given to frontends**: `ParticipantSnapshot::thread`, a `thread` on a
  history page item (the store's `KeptItem::thread` is there already), so that a frontend sorts by
  thread instead of guessing.
- **Writing into a thread**: who may, and whether the sub-agent is then asked to respond as by its
  delegate.

## Done when

- A frontend shows a thread of a channel, and the decisions on writing into it and on waking and
  stopping it alone are recorded here.
- What a frontend is given says its thread where it needs it.

## Working apart with an agent

Moved here from issue [596](596-a-user-cannot-start-a-sub-agent-by-command.md) on 2026-11-23: a
user who wants an agent to work on something apart from the line, reading along and asking again,
wants no sub-agent, whose thread is a closed circle delegated into, but a thread with that agent
in which they write themselves. The agent is made by `/agent create` or brought in, and may take
part in that thread alone (issue 602). Missing: how a user opens such a thread with an agent, and
the view of it - to be decided with writing into a thread.

## Plan

Proposed by the author on 2026-11-23, after 602, and decided by the maintainer on the same day,
step 1 cut anew (History).

### What the kernel has, and what the frontends lack

Traced on 2026-11-23:

| What | Kernel | REST | Web UI, TUI |
|---|---|---|---|
| Reading a thread | its history by thread | `?thread=` on a page | the line alone |
| The thread of an item, an event | `KeptItem::thread`; `EventContext::thread_id` | events carry it, items of a page not | events sorted into the line or out |
| The threads of a channel | `ChannelLoop` holds them | none listed; `thread_opened` items | none |
| Who takes part where, a turn's thread | `ChannelActor::threads`; `Actor::turn_under_way(thread)` | the snapshot's `turn_under_way` is the line's alone | none |
| The thread of a waiting call | its turn's | `pending_approvals`, `pending_questions` say none | none |
| Writing into a thread | admitted unless its circle is closed | `thread` on `POST …/messages`, `…/commands` | the line alone |
| Opening a thread | `procedure::thread::Open`, `OpenThread`, founders; a user at the line alone | none | none |
| Stopping, waking one thread | `ChannelLoop::cancel`, `wakeup` take a thread | `POST …/cancel` and `/agent wakeup` pass none | none |

### Steps

1. **An item of a page says its thread** (`HistoryPageItem`, below): the terminal no longer takes a
   message of another thread on a page for the answer `/retry` asks for.
2. **The web UI shows a thread** (E1), given what it needs for it (interface below): its messages,
   calls and turns, read and followed as the line is, and written into where its circle is open
   (E2). A call shown from another thread no longer
   stays shown as running: once the client keeps what it shows per thread, a result reaches its
   call whatever the line's view shows - a question of the client, not of a field.
3. **Stopping and waking one thread** (E3): `POST …/cancel` names a thread, `/agent wakeup` wakes
   the thread it is issued in.
4. **A user opens a thread** (E4): `POST …/threads` at a message of the line, with its founders,
   and in the web UI; *Working apart with an agent* above.
5. **The terminal follows a thread** (E5).

### To decide

- **E1 - Where the web UI shows a thread.** (a) In a pane beside the line, opened at the message it
  hangs at, which shows that a thread hangs there - who takes part, how many messages, whether
  someone answers -; on a narrow screen in place of the line. (b) Inline under that message,
  folded. (c) As an entry of its own in the list of channels. Proposed: (a) - the line stays
  readable, a thread is followed beside it, and the message it hangs at is where 557 and 558 want
  it shown.
- **E2 - Writing into a sub-agent's thread.** Its circle is closed: nobody outside says anything
  in it (`Circle::Closed`, `closed_circle` on `POST …/messages`), since the delegate takes the
  sub-agent's next turn end as the answer to its brief (issue 558, *How it works*; 602). Letting
  users write would reopen both. Proposed: users read a sub-agent's thread and stop it (E3); what
  it should do otherwise they tell the agent that started it.
- **E3 - What stopping and waking reach.** (a) In a thread's view, that thread; in the line's, the
  whole channel - as today, and as 558 decided a cancel of the channel to stop everything. (b) The
  line alone from the line's view, the channel by a separate act. Proposed: (a). It touches 602's
  open question whether `ChannelLoop::cancel_actor` is to end an actor's turns set aside in other
  threads.
- **E4 - Who opens a thread, with whom.** The kernel lets a user open one at a message of the line
  alone, asking `OpenThread`, with founders who are the user or agents of theirs; a founder new to
  the channel takes part in the thread alone (602), and an open circle draws in the author of the
  message it hangs at. Proposed: a user opens threads with an open circle alone - a closed one is
  for delegating -, with themselves and agents of their own as founders, among them one just made
  by `/agent create`: *Working apart with an agent*.
- **E5 - The terminal.** (a) It follows one thread at a time, chosen by a command of the terminal
  that lists the open threads and switches. (b) It stays with the line, and a thread is followed
  in the web UI alone. Proposed: (a), as its own step after the web UI's.
- **E6 - The shape of what a frontend is given** (below): the turns of a participant per thread,
  the thread it answers in now, and where the thread of a waiting call is said.

### Interface of step 1

```rust
// caelumora-domain: HistoryPage::items is a Vec<HistoryPageItem>
/// An item of a `HistoryPage`: an entry as its reader sees it, where it stands and where it
/// happened.
pub struct HistoryPageItem {
    pub seq: Position,
    /// The thread it happened in; `None` for what happens in the whole channel.
    pub thread: Option<ThreadId>,
    pub entry: HistoryEntry,
}
```

### Interface of step 2

```rust
// caelumora-rest-api: an item of a page names its thread beside its position:
// {"seq": 7, "thread": "…", "said": …}
pub struct HistoryPageItem<M> {
    pub seq: Position,
    /// The thread it happened in; `None` for what happens in the whole channel.
    pub thread: Option<ThreadId>,
    pub item: HistoryItemResponse<M>,
}

// caelumora-domain: ChannelParticipantSnapshot, in place of `turn_under_way` (the line's alone)
    /// The threads of the channel the participant takes part in (issue 602).
    pub threads: Vec<ThreadId>,
    /// The turn under way in each thread - the one it works on, or one set aside waiting for a
    /// person (602) -, of an agent run here in a running channel.
    pub turns_under_way: BTreeMap<ThreadId, TurnId>,
    /// The thread of the turn it works on now, if any: where it answers.
    pub answering_in: Option<ThreadId>,

// caelumora-domain: ChannelSnapshot
    /// The threads of the channel that are open, its line not among them (issue 557).
    pub threads: Vec<ThreadSnapshot>,

/// An open thread of a channel as the viewer sees it.
pub struct ThreadSnapshot {
    pub id: ThreadId,
    /// Where it hangs, who opened it, whether who takes part may grow.
    pub opening: Opening,
    /// To whom it was delegated, if it was: a sub-agent's thread (issue 558).
    pub recipient: Option<ActorId>,
}

// caelumora-domain: ApprovalRequest and QuestionRequest
    /// The thread of the turn whose call waits for it.
    pub thread: ThreadId,

// caelumora-api: Actor
    /// The thread of the turn it works on now, if any - not one set aside.
    fn answering_in(&self) -> Option<ThreadId> { None }
```

Kept up by the events that follow a snapshot: `ThreadOpened` and `ThreadClosed` the channel's
threads, joins and leaves of a thread a participant's, `StateChanged` and the turns' ends where it
answers. To check before building: that the channel can answer `answering_in` from what the
actor's run control holds, and whether the thread of a waiting call is better said on the request -
which its event already carries in its context - or only in the snapshot.

## History

- **2026-11-18** — Opened from issue 557, step 6 of 2b, and 558's open points: the view of a thread
  left for later, and what its review found missing.
- **2026-11-23** — *Working apart with an agent* moved here from 596: a thread with an agent in
  which a user writes themselves, no delegation.
- **2026-11-23** — *Plan* proposed, after 602: what the kernel has and the frontends lack, five
  steps, E1 to E6 to decide, and the interface of step 1. Not decided.
- **2026-11-23** — E1 to E6 decided by the maintainer, as proposed: a thread in a pane beside the
  line; no writing into a sub-agent's thread; stopping and waking a thread from its view, the whole
  channel from the line's; users open threads with an open circle, with themselves and agents of
  their own as founders; the terminal follows one thread at a time, as a step of its own; the
  interface of step 1 as written.
- **2026-11-23** — Step 1 cut anew by the maintainer, on tracing it: a snapshot's `turn_under_way`
  is the line's alone since 602, so a sub-agent in a turn is no longer shown as answering in the
  line; the rest of step 1's interface - a participant's threads, turns and `answering_in`, the
  channel's threads, the thread of a waiting call - has no user before the web UI's view of a
  thread, and moves to step 2, with `Actor::answering_in`, which the channel needs to tell where
  an agent answers (its run control alone knows whether a turn runs or was set aside). Step 1 is
  the thread of an item of a page.
- **2026-11-23** — Step 1: an item of a `HistoryPage` is a `HistoryPageItem` - its position, the
  thread it happened in (`Recorded::thread`), its entry -, as the stores read it (`seen`,
  `page_of`), named as the REST API's item is, on review; the terminal takes only an agent's answer
  of the line on a page as what `/retry` asks for, as it does live.
- **2026-11-23** — Step 2 decided by the maintainer, on tracing the web UI: `ChatView` holds one
  conversation, and a single handler of the stream mixes what changes it with what changes the
  whole channel. A conversation becomes a part of its own - a pure function applying an event to
  its state, and a view of its list, its pages and its input -, which the line and a thread's pane
  both are; `ChatView` keeps what is the channel's and hands each event on once. Not a second list
  beside it, which would repeat the conversation's logic. Step 2 is cut in two: 2a takes the
  conversation out of `ChatView` without a change of behaviour; 2b shows a thread with it, with
  what the backend is to give for it (*Interface of step 2*). One thread is open at a time; for
  the others the channel keeps what the mark at the message they hang at needs. The pane takes the
  place of the channel's side panel, with a width of its own, and on a narrow screen that of the
  chat. Planned beside the steps, as step 6, after 2b: **what is read in a thread** - where a user
  read up to in it, what is new in it at its mark, and how that counts in the channel's unread -,
  to be decided as it comes; until then, what is read stays the channel's and follows the line.
- **2026-11-23** — Step 2a: the conversation taken out of `ChatView` in the web UI.
  `conversation.ts` holds one conversation - its messages, its answers under way, where its earlier
  pages are read - and how an event of the stream (`withEvent`) and a page of the history change
  it; `ConversationView` shows it whole: its list, what waits for the user between the list and the
  field to write in (`children`), the field (`Composer`, its state `useComposer`) and the files
  dropped onto it. It reads the channel from a context of the chat, `ChatChannel`, beside
  `ShownChannel`. `ChatView` keeps what is the channel's and hands on what the stream says of the
  line. Decided on review, beside the interface agreed: the field is part of the conversation's
  view, so that the list follows the user's own words without a signal across components; a
  message read back replaces one shown (`withReplaced`), one announced elsewhere is added once
  (`withAdded`); the context named `ChatChannel` for what it is. One change of behaviour, as
  agreed: an event of another thread no longer ends an answer shown in the line. Keyed by the
  channel, the conversation keeps the draft typed in one channel out of another.
