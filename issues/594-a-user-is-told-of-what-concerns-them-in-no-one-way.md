# 594 — A user is told of what concerns them in no one way

**Status:** in progress
**Type:** question
**Priority:** medium
**Area:** User interaction
**Summary:** What concerns a user - a mention, an approval, a question, a message in a channel they do not look at - is told by each frontend in a way of its own, decided case by case: the web UI by its unread count and a browser notification for a mention, the terminal by "elsewhere", neither by one rule, so that with threads (issue 557) a mention in a sub-agent's thread, an `@all` written there, or channels followed side by side are told inconsistently or not at all.

## The problem

What a user is told of, and where, has grown with each feature:

- **The web UI** counts what is new in a channel from the snapshot on and with every `SAID` the
  global stream announces (issue 404), marks a mention with an `@`, and shows a browser
  notification for a message that mentions the user, where they allowed it. Approvals and
  questions are shown in the chat of the channel they belong to, nowhere else.
- **The terminal** prints what concerns the user wherever they talk (`Followed::concerns_user`):
  an approval, questions put to them, a message that mentions them, a call they made - in another
  channel as "elsewhere". The full-screen view tells nothing of agents at work in other channels
  ([572](572-the-full-screen-terminal-tells-nothing-of-other-channels-at-work.md)).
- **Threads** (issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md)) added a view of a
  channel's line that leaves other threads out, but for their approvals, questions, and what
  mentions the viewer (`Envelope::is_shown_in_line`, `Filter::shown_in_line`). Decided on
  2026-11-18 as a first step: a mention in another thread is shown in the web UI.

What is not decided:

- **`@all` in a thread.** Mentions resolve against the whole channel's directory, so a sub-agent
  writing `@all` in its thread is shown in every user's line, counted as unread with a mention, and
  raises a browser notification for everyone.
- **Which events concern a user at all**, and whether that is one rule of the kernel - a predicate
  over an envelope and a viewer, as `is_shown_in_line` is - or each frontend's.
- **How the terminal tells** what concerns the user in channels followed side by side (572).
- **What a notification is**: a count, a mark, a browser notification, a line elsewhere - and
  whether a user chooses what they are told of.

## Why it matters

With sub-agents working in threads, more happens where the user does not look. If what concerns
them is told by rules spread over the frontends, every new source of events - a thread, a
scheduled agent, another channel - is told differently in each, or forgotten in one.

## Considerations

- **One predicate in the kernel** (`concerns(viewer)` beside `is_shown_in_line(viewer)`), which
  every frontend asks: the same rule live and, as a `Filter`, for counting what was missed.
- **Each frontend decides**, as today: simple, but the rules drift apart.
- **`@all` in a thread** reaching only who takes part in that thread, or the line's users only when
  the thread is the line - a change of how mentions resolve, not of the frontends.

## Done when

- What concerns a user is decided - which events, told how, by which rule - with the reasoning
  recorded here.
- The steps are planned here, or in issues of their own; 572 is taken into account.

## What exists, as of 2026-11-23

Two concepts are mixed in the problem above. **Shown** is what a view of a channel holds
(`Envelope::is_shown_in_line`, `Envelope::is_shown_in_thread`). **Concerns** is what is the
user's business wherever they look (`Followed::concerns_user`): what the view of the thread followed
shows, narrowed - questions only if put to them, an answer to an approval only if theirs, a call
only if they made it.

The rule of what is shown or concerns a user is kept in five places:

| Place | What it decides |
|---|---|
| `Envelope::is_shown_in_line`, `Envelope::addresses` (`caelumora-domain`) | what the line's view shows live |
| `Filter::shown_in_line` (`caelumora-domain`) | the same over the history: pages, what is unread |
| `addresses_a_user` in `ChannelLoop::record` (`caelumora-kernel`) | what the global stream announces as `said` |
| `isShownInLine`, `isSaidInLine` (`caelumora-web-ui/src/messages.ts`) | the first, again in TypeScript |
| `Followed::concerns_user` (`caelumora-tui`) | what the terminal tells of a channel not talked in |

What each frontend tells of a channel the user does not look at:

| Event | Web UI | Terminal |
|---|---|---|
| A message addressing the user | unread count with `@`, browser notification | told "elsewhere" |
| Another message in the line | unread count | nothing |
| An approval requested | nothing | told |
| Questions put to the user | nothing | told (`Questions::is_asked`) |
| Questions to others, of another thread | shown in the line's view (`is_waited_for`) | not told |
| A call of the user's, an approval answered by their rule | - | told: feedback on what they did |

**`@all` in a thread.** Since issue 602, `Message::addressees` holds everyone a group handle
addresses, resolved against the whole channel (`MentionDirectory`), and `Message::mentions` those
mentioned by name. Routing is already the thread's: a thread's moderator knows only the agents
taking part in it (`PolicyModerator::mentioned_responders`). Drawing in uses the names alone (issue
602). Showing, counting and notifying use `addressees`, and so the whole channel: a sub-agent's
`@all` reaches every user.

## Decision

Decided by the maintainer on 2026-11-23, as the author recommended.

**D1 - `@all` in a thread addresses who takes part in that thread.** A group handle (`@all`,
`@channel`) is resolved against the participants of the thread the message is said in, not against
the whole channel. `Message::addressees` then means the same as in routing, and everything that
reads it - `Envelope::addresses`, `Filter::addressing`, the unread count, the browser notification,
`said` - follows without a change of its own. Rejected: letting another thread reach a viewer by
name alone (`Message::mentions` instead of `addressees` where a view shows another thread) - it
would give "addressed" two meanings, and a user taking part in the thread would not be told of an
`@all` said there while looking elsewhere. `MentionDirectory` needs everyone per thread, which the
attendance per thread of issue 602 gives. Messages stored keep their `addressees` as resolved when
they entered; nothing is migrated.

**D2 - What concerns a user is one rule of the domain.** What concerns them:

- an approval requested, from its request to its end - the request does not name who may approve
  it, so, as in the terminal today, every request of a channel concerns its users;
- questions put to them;
- a message heard that addresses them (after D1).

The feedback on what a user did themselves - the result of a call they made, an approval answered
by their rule - does not concern them in this sense: it belongs to the terminal's one line of input
across channels and stays in `Followed`. The rule is a predicate beside `is_shown_in_line`:

```rust
/// Whether it concerns `viewer` wherever they look: an approval, questions put to them, a message
/// heard that addresses them.
pub fn concerns(&self, viewer: ActorId) -> bool
```

`Followed::concerns_user` builds on it. What a user missed needs no `Filter` of its own: approvals
and questions are no items of a page but state (`pending_approvals`, `pending_questions` of the
snapshot), so what was missed is the unread messages addressing them (`Filter::addressing`) and
what is pending now. The web UI cannot ask a predicate of the kernel, and `said` reaches every
reader alike: it keeps a TypeScript mirror of the predicate, as `isShownInLine` is one, documented
as such and tested with the same cases. A stream filtered per subscriber was weighed and found out
of proportion. (Changed on 2026-11-24: no mirror, see *History*.)

**D3 - How it is told is each frontend's.** The rule of D2 says what concerns a user; how each
frontend tells it is its own. The web UI marks a channel where something waits for the user - an
approval, questions put to them - in the session list, beside the unread count and the `@`, and
shows a browser notification for an approval and questions put to the user as for a mention. The
terminal tells as today, through the predicate. A choice of the user's of what they are told of -
muting a channel - is not built: nobody asks for it yet.

**D4 - 572 stays apart.** Issue 572 is about activity - agents at work in another channel -, not
about what concerns the user. It stays an issue of its own; of this one it can take a mark of the
channels where something waits for the user, in the full-screen view's status line.

For agents, what concerns them is what their moderator asks them; no predicate of their own is
needed. `concerns` takes an `ActorId` and leaves them out of nothing.

## Plan

Each step a commit of its own, reviewed before the next.

1. **`@all` in a thread.** Group handles resolved against the participants of the message's thread
   (`MentionDirectory`, where the channel resolves mentions as a message enters), with tests in the
   domain and the channel.
2. **What concerns a user.** `Envelope::concerns`; `Followed::concerns_user` built on it; the
   TypeScript mirror; the web UI marks what waits for the user in the session list and notifies of
   approvals and questions put to them. `addresses_a_user`, which decides what `said` announces, is
   aligned with the rule. Changed on 2026-11-24 (see *History*) to two steps, without the
   mirror:
   - **2a - the rule.** `Envelope::concerns`: an approval from its request to its end, questions
     put to the viewer and their closing where shown to them, a message heard that addresses them
     (`Envelope::addresses`). `Followed::concerns_user` built on it, keeping what is the
     terminal's own: the thread followed closing, the feedback on a call or an approval answer of
     the user's.
     `addresses_a_user`, which decides what `said` announces, moved to the domain as
     `Message::addresses_a_user`.
   - **2b - what waits for a user, in the web UI.** The `UserActor` keeps what waits for its user
     in its channel: the open questions put to them, as it keeps them already, and now the
     approvals requested, from their request to their end. A channel's snapshot gives it to the
     viewer (`ChannelSnapshot::waiting`, a `Waiting` of approvals and questions counted, with
     `Detail::WAITING`), read from the viewer's running actor; as it changes, the actor announces
     `waiting_changed`, carrying the user's `ActorId` alone. The session list fetches the session
     again on it, marks the channels where something waits, and notifies where a count rose.

572 is built apart, whenever it is taken up.

## History

- **2026-11-18** — Opened from issue 557, as the maintainer asked: notifications as a whole to be
  taken up apart, the terminal's with a concept of its own together with 572.
- **2026-11-23** — What exists assessed, and D1 to D4 decided by the maintainer, as recommended
  (*Decision*): `@all` in a thread addresses who takes part in it; what concerns a user is one
  predicate of the domain, `Envelope::concerns`; how it is told is each frontend's; 572 stays
  apart. The steps planned (*Plan*).
- **2026-11-23** — Step 1 done: a group handle is resolved against who takes part in the thread
  the message is said in (`MentionDirectory::resolve` given them, `ChannelLoop::taking_part_in`),
  of a modification the thread the message was said in; a tool's questions to `@all` still go to
  every user of the channel (`MentionDirectory::lookup`). Decided by the maintainer in its review:
  a group handle never addresses the message's sender (`Message::resolve_mentions`), so that
  whom it addresses does not depend on whether the message draws its sender into the thread
  first; and a modification resolves against who takes part in the thread as it is modified, as
  its names are resolved anew then.
- **2026-11-24** — Step 2 split in two, and its source for the web UI decided by the maintainer
  (*Plan*). The global stream tells nothing of approvals and questions, so the TypeScript mirror of
  `concerns` that D2 foresaw would have nothing to run on for a channel not open; it is not built,
  the kernel tells what waits for a user instead. Weighed: the channel loop keeping what waits, from
  the requests it records - a third keeper of pending approvals beside the turn and the approver,
  and the rule a second time where the view counts it per viewer; reading the agents' stored
  state, which lags - the agent sends its requests before it stores them (`request_approvals`), so
  a mark would come late or stay; announcing the requests on the global stream - it reaches every
  reader of the channel, where sensitive questions go to those asked alone. Chosen: the
  `UserActor`, which is given what is delivered to its user and keeps the questions put to them
  already.
- **2026-11-24** — Step 2a done: `Envelope::concerns`, `Followed::concerns_user` built on it,
  `Message::addresses_a_user` in the domain. What the terminal tells of a channel not talked in
  changes in two ways: the end of an approval and the closing of questions concern the user - it
  prints nothing of them, as before -, and a message not heard (`Message::is_heard`) that
  addresses them is told there no more, as the web UI leaves it out. Reworded with it, as the
  maintainer asked: what a view shows of another thread is what *addresses* the viewer, not what
  mentions them (issue 602), and the rule every channel delivers by says whom an event *goes to*,
  where it said whom it concerns.
