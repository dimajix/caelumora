# 236 — A remote agent is not told when a message it was given is modified or taken back

**Status:** done
**Area:** Integrations
**Summary:** A remote agent is not told when a message it was given is modified or taken back, so it goes on answering the old version.

## The problem

An external agent ([179](179-external-agents-via-a2a.md)) keeps what it heard in its transcript
until it is next asked, then gives it to its remote agent, and its transcript starts anew
([205](205-external-agent-message-context.md)). Since [234](234-modify-revoke-and-retry-messages.md)
a user can modify or take back a message. The external agent records the change in its history,
and in its transcript as long as the remote agent was not given the message yet. Once it was, the
change reaches nobody: the remote agent keeps the old version and goes on answering to it.

## Why it matters

Correcting one's message is meant to stop the old version from acting. For an agent that runs
elsewhere it does not, and nothing tells the user so - in a channel with agents of both kinds the
same correction works for one and not for the other.

## Considerations

- The remote agent keeps its conversation; A2A has no way to change what was sent. What can be done
  is telling it, with the next call, as text: "alice corrected an earlier message: ...", "alice
  took back an earlier message". A prompt part of the external agent's personality, given like the
  roster (`changes_only`), would fit the pattern of [204](204-external-agent-roster.md).
- Taking back cannot be undone remotely; the text can only ask the remote agent to disregard it.
- Retry is rejected by external agents (`RetryRefused::Elsewhere`) for the same reason; whether a
  remote protocol can ever offer it is a question of its own.
- **What A2A 1.0 offers** (checked against the specification): no method changes, takes back or
  asks again for anything sent - the methods are `SendMessage`, `SendStreamingMessage`, `GetTask`,
  `ListTasks`, `CancelTask`, `SubscribeToTask`, the push configurations and
  `GetExtendedAgentCard`. A task that ended takes no further message; the conversation goes on
  with a new task in the same `contextId`. `referenceTaskIds` names earlier tasks, not messages.
  `metadata` and `extensions` could carry a change only by an extension both sides speak.
- **What Hermes offers** (its A2A plugin, `plugins/platforms/a2a/`): a `contextId` is a session of
  its gateway; of a message only the text of its parts is read - `metadata`, `extensions` and
  `referenceTaskIds` are not, and a `taskId` sent is not either: every message begins a task of its
  own. Every text that comes in is framed as untrusted input of a peer, `/`-text too, so that a
  peer never reaches the operator's commands: its `/retry` and `/undo` are deliberately out of
  reach. Its injection filter replaces, among others, `disregard (all|any|the) (previous|prior|
  above)` with `[filtered]` - the wording of a notice would matter. `CancelTask` drops the answer
  but does not stop the turn under way, and a context takes `A2A_MAX_PINGPONG_TURNS` calls (5 by
  default, 20 at the most) until it was idle for an hour or a task of it was cancelled; after that
  every call is rejected.
- **Retry over A2A** could only be had by a new `contextId` given the whole conversation as it
  stands - a conversation begun anew, in which the remote agent loses its own state while the old
  one stays with it, and Hermes's memory reaches across sessions all the same - or by asking in
  text, which leaves the old answer standing. `RetryRefused::Elsewhere` stays.

## Plan

The remote agent is told of a change to a message it was given by a *notice*: a message made here,
in the name of the change's author, that says what became of the earlier one - "[changed an earlier
message "..."; it now reads:] ...", "[took back an earlier message "..."; it no longer stands.]". It
is appended to the external agent's transcript as any message is, so it is given with the next
call, at its place among what was said since, and the transcript records it word for word: what a
call was given can be reconstructed as it was ([205](205-external-agent-message-context.md)). A
notice only asks: the remote agent - Hermes takes it as untrusted input of a peer - decides whether
the old version stops acting.

### What was weighed and not chosen

- **A prompt part** given like the roster ([204](204-external-agent-roster.md), `changes_only`): it
  would have to be configured on every external personality, and it stands before what was said,
  not at its place among it.
- **Working the notices out when the call is made**, from the history: nothing would record that
  the remote agent was told, and the context of what comes of the call would no longer say what it
  was given.
- **An extension of A2A, or `metadata`:** only a remote agent that speaks it would understand it;
  Hermes reads nothing but the text.
- **Recording the change as an entry** (`Modified`, `Revoked`) on the active path, and wording the
  notice whenever the path is given or resolved - the first plan, built on a branch that was
  abandoned. The transcript would record what happened rather than what was given, and its digest
  would cover the change but not the text: once the wording changed, the context of an earlier
  answer would show a notice its call was never given. It would also have needed a second rule
  beside `as_they_stand` for what is given, used by the call and by the `ContextResolver`.

### The model (`caelumora-model`)

Only what the notice needs, none of the decision: whether and how a change is told is the external
agent's alone. `AgentChannelState::modify_message` and `revoke_message` go: they recorded a change
in both chains and said only whether either took it, so whoever had to tell what the transcript did
not take asked the transcript a second time. Now `take_in` records a change in the history and the
transcript itself, and the retry of an agent run here takes its answer back in both chains itself.

- **`Message::notice_of: Option<MessageId>`** marks a notice and names the message it tells of. A
  notice has an id of its own - with the id of the message, every later change of that message
  would apply to the notice too - is said by nobody, and is never in a history: a reader of the
  transcript that takes a message not internal for one the history holds as well
  (`given_up_turns`) passes it over. Not covered by the digest of the message chain: the content
  already says what was given.
- **`Transcript::message(id)`** finds a message on whichever branch holds it, as it stands there -
  a query, as `MessageHistory::message` is: where `messages` gives only the active path.
- **`TranscriptEntry::classification`** says how an entry classifies what it records of a message,
  for the label of a call (below).

### The external agent (`caelumora-runtime`)

- **`take_in`** records a modification or revocation in the history and the transcript where they
  hold the message - the transcript on its active path - and hands one the transcript did not take
  to a function of the actor, where it came, so in order with what was said: as an entry of what
  became of the message as the agent hears it, a modification of which nothing passes its perceive
  processors as taken back. One decision, the transcript's (`modify` and `revoke` say whether they
  recorded), read once. An agent run here passes a function that does nothing.
- **`tell`**, the external agent's: while it keeps a remote conversation, of a message it was given
  (`Transcript::message`) and that was not taken back where it was given, it appends a notice to
  the transcript - or, if a notice of the message was not given yet, modifies that one, so that
  several changes before the next call are told once, as the message stands then.
- **`notice(was, now, at)`** words it, in one place, in English: the message as it was first given
  (`was`), named by what it said, cut to some 200 characters, and what became of it (`now`:
  modified to it, or `None`: taken back). Its sender and role are the message's, so that it is
  marked as every message is (`sender_format`) and names nobody itself; it is classified as the join
  of what it quotes and what it says, without the quote where the two have no audience in common.
  No instruction to disregard anything, which a filter takes for an injection - Hermes's turns
  `disregard the previous` into `[filtered]`.
- **`exchange`** stays as it is: a notice is a message of the transcript, given with `messages`, and
  a request to respond after which only a notice is new still makes a call. A notice alone makes
  none; it waits for the next request.
- **The label of the call** sees every entry's classification (`TranscriptEntry::classification`):
  a message's, a modification's and a reclassification's - today only the messages as they were
  first recorded are seen, so a message raised by a modification before it was given labels the
  answer too low. Seeing every change only raises the label, as the remote agent may have used
  either version.
- No new action acts in anyone's name: the notice is given in the call, under the request's
  security context, as what was said is.
- **Reclassifying** is not told: the remote agent knows no labels.

### Documentation

AGENT-TURN.md (*Changes to a message*), CHANNELS.md (*Changing what was said*) and the passage on
external agents in ARCHITECTURE.md say that a remote agent is told of a change to a message it was
given, by a notice with its next call, and that it is asked, not made, to follow it.

### What is left out

- **Reclassifying** and **lowering a channel** are not told (see above).
- **Retry** stays rejected (`RetryRefused::Elsewhere`).
- **A remote agent that opened no conversation** is given the conversation anew each time and is
  told nothing; **one that opens another conversation** than the one it had is still told of a
  message given in the old one.
- **A notice is text:** a participant can write the same into a message of their own. The sender
  marker names who wrote it, but a forged notice can claim to change "an earlier message" - a case
  of [150](150-impersonation-towards-the-agent.md).
- **The quote** is the message as it was first given: after a change told already, a second notice
  names it by its first version.
- **Hermes's limit of calls per context** (`A2A_MAX_PINGPONG_TURNS`) is not dealt with here.

### Steps

One step: the model's pieces, `take_in` handing changes over, the external agent's `tell` and
`notice`, the label; tests of the notice, of `tell`, of `take_in` handing over in order, and a
channel test of a modification and a revocation reaching the remote agent and the context of its
answer; documentation; status done.

## Done when

A remote agent is told, with its next call, of every modification and revocation of a message it
was given, and the documentation says what that can and cannot achieve.

## History

- **2026-10-06** — Opened from open question 6 of [234](234-modify-revoke-and-retry-messages.md).
- **2026-10-06** — Rejected. A2A 1.0 and Hermes were checked (see *Considerations*): neither offers
  a way to change, take back or ask again for what was sent, and Hermes reads nothing but the text.
  A notice in text is all that could be done, and it would only ask - the remote agent, which takes
  it as untrusted input of a peer, decides whether the old version stops acting. A correction
  reaches a remote agent only as long as it was not given the message yet; CHANNELS.md says so, and
  why.
- **2026-10-06** — Reopened. Even if a notice can only ask, the remote agent should learn that a
  message it was given was changed or taken back - as something said of its own, given with the
  next call.
- **2026-10-06** — Planned (see *Plan*): a notice, said in the name of the change's author, recorded
  in the external agent's transcript and given with the next call.
- **2026-10-06** — Plan revised (see *Plan*, *What was weighed and not chosen*): the notice is a
  message of its own (`Message::notice_of`) appended to the transcript, so that it is recorded as it
  was given. The first plan was built on the branch `issue-236-external-agent-told-of-changes` and
  abandoned there; this one is built afresh from main.
- **2026-10-06** — Plan revised again, before anything was committed: the decision to tell a change
  is the external agent's alone, so the model keeps only what a notice needs (`Message::notice_of`,
  `Transcript::message`) and `AgentChannelState` stays as it was. `take_in` hands every change over
  where it takes it in rather than returning it: returned, a notice came after what was said in the
  same batch after the change. Whether the change is one to tell - of a message given already - is
  for `tell` to see. Done in one step: the external agent's `tell` and `notice`, the label
  seeing every entry's classification, tests, AGENT-TURN.md, CHANNELS.md and ARCHITECTURE.md.
- **2026-10-06** — Whether the transcript took a change was decided twice, by `Transcript::revoke`
  and again by `tell` with a predicate of its own; now `take_in` records in both chains itself and
  hands over only what the transcript did not take, and `AgentChannelState::modify_message` and
  `revoke_message` are gone - and `reclassify_message` with them, which had the same form and the
  same one caller.
