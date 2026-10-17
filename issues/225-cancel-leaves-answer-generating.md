# 225 — After a cancel, the web UI shows the answer as still being generated

**Status:** done
**Area:** User interaction
**Summary:** After a cancel the web UI keeps showing the answer as being generated, and the cancel button stays lit.

## The problem

Reported by the maintainer: the cancel button no longer works well. The agent does not seem to be
cancelled, the button stays lit, and "generating…" stays in the chat view.

The agent is cancelled: its turn ends, is taken over, and it announces the end with a
`FullResponseGenerated` (`AgentActor::end_turn`) - with no messages if it was cancelled before it
said anything, which is what a cancel during a model call leaves, since streamed pieces are not
kept. The web UI shows an answer as under way from its `ResponseStarted` until the
`FullResponseGenerated` of the same sender (`ChatView.tsx`: `isGenerating` is `answers.length > 0`,
and `answerEnded` removes the answer); the cancel button and "generating…" follow it.

Since issue [220](220-user-stream-from-its-actor.md) the web UI follows the user's actor, which
receives what the moderator routes to it. The moderator forwarded a `FullResponseGenerated` only if
something of it was left after shaping it for the others (`as_user_messages`), and otherwise to
nobody - to the agents, who had nothing to hear, but also to the users, who need the end. Before
220 the web UI read the channel's broadcast, which had it. A response with nothing to hear never
reached the web UI:

- a turn cancelled before it said anything - the report;
- a turn that ends with tool calls only, e.g. `stop` without text;
- an external agent's answer with nothing in it - cancelled before anything came, or nothing new
  to give.

Confirmed with a probe: after a cancel, a user's actor received nothing of the agent within a
second, although the agent was idle again.

## Done when

- A user learns that an answer ended however little it said, the web UI clears it, and the cancel
  button goes out.
- A test shows a user's actor receiving the end of a turn cancelled before it said anything.

## History

- **2026-10-04** — Issue created at the maintainer's report. The cause had just been found while
  working on [224](224-what-agents-hear-of-others.md): a regression of 220. Fixed with 224's step
  3: the moderator forwards a response as it was said, to every user always and to the agents if
  there is something to hear (`Message::is_heard`); a test in `channel.rs` follows a user's actor
  through a cancel. Not checked in the browser.
