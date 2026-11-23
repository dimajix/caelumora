# 572 — The full-screen terminal tells nothing of the agents at work in other channels

**Status:** open
**Type:** feature
**Priority:** low
**Area:** User interaction
**Summary:** In `caelumora-tui`'s full-screen view the status line shows only the answers under way in the channel talked in, so whoever follows several channels of a session does not see that an agent works in another one - the line-wise view at least tells that one began to answer there.

## The problem

Issue [569](569-the-terminal-mixes-its-output-into-the-line-typed.md) gave the terminal two ways
of drawing. Both follow every channel of the session the user takes part in (issue
[561](561-the-terminal-shows-one-channel-of-a-session.md)), and of a channel not talked in they
print what concerns the user - an approval, questions put to them, a message mentioning them.

*   **`line`** also tells, marked with the channel's name, that an agent began to answer there
    ("[side] helper is answering...").
*   **`full`** draws nothing of it: an answer begun is shown by the status line, and the status line
    shows the answers under way of the channel talked in alone. That an agent works in another
    channel - thinks, runs tools, waits for an approval there - the user does not see until it
    says something that concerns them.

Each channel followed keeps its answers under way already (`Followed::under_way`), from the same
events; only what the screen is given and shows is limited to the channel talked in
(`Channels::under_way`).

## Considerations

To be decided before it is built:

*   **Where it shows:** in the status line, after the answers of the channel talked in, each marked
    with its channel (`[side] helper is running tools · 12s`); or summed up per channel
    (`side: 2 at work`) where the line would grow too long; or in a line of its own.
*   **How much:** every answer under way in every channel followed, or only the agents' state
    without their streamed text - which stays the channel talked in's.
*   **`line`:** whether it tells more than that an answer began, which it should not do at every
    change of state, for a screen reader reads every line.

## Done when

- The full-screen view shows, as decided, that agents work in other channels the user follows.

## History

**2026-11-14** - Opened from the review of step 3 of issue 569, on the maintainer's wish to see
something of the activity in other channels.

**2026-11-18** - Issue 557 decided that the terminal's notifications get a concept of their own,
together with this issue: since 557 the line's view (`Envelope::is_shown_in_line`) shows the
terminal of another thread its approvals, its questions and what mentions the user, told
"elsewhere" in another channel. What concerns a user as a whole is issue 594.

**2026-11-23** - Issue 594 decided what concerns a user (`Envelope::concerns`) and kept this issue
apart from it: this one is about activity; of 594 it can take a mark of the channels where
something waits for the user.
