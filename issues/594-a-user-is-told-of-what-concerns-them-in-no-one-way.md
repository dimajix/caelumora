# 594 — A user is told of what concerns them in no one way

**Status:** open
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

## History

- **2026-11-18** — Opened from issue 557, as the maintainer asked: notifications as a whole to be
  taken up apart, the terminal's with a concept of its own together with 572.
