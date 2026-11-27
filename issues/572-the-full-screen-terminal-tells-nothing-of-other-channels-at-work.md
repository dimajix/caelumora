# 572 — The full-screen terminal tells nothing of the agents at work in other channels

**Status:** done
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

## What exists

Checked against the code on 2026-11-25:

*   **`line` tells nothing either.** Since 594, a channel not talked in prints only what concerns
    the user (`Channels::take_in_shown`, `Followed::concerns_user`); an answer begun
    (`ResponseStarted`) does not, so "[side] helper is answering" is no longer printed.
*   **Other threads are left out too.** `Followed` keeps the answers under way of the thread
    followed alone: `read_under_way` reads `turns_under_way` of that thread, and of another thread
    only the end of a turn is taken in (`take_in_of_another_thread`). A sub-agent at work in its
    thread ([558](558-no-agent-hands-a-task-to-a-sub-agent.md)) is not shown, not even in the
    channel talked in.
*   **Where an agent stands is lost in a thread followed.** What an agent says of itself outside a
    turn's thread - `StateChanged` - happens in the channel's line, so the view of another thread
    does not show it, and the answer under way there keeps its first state.

## Decision

Decided by the maintainer on 2026-11-25, as recommended:

*   **D1 - Where.** Every other place followed counts: the other channels, and the other threads of
    every channel followed, the one talked in included.
*   **D2 - How.** In the status line, after the answers of the thread followed: per other channel a
    short mark - `[side] helper` for one agent at work, `[side] 2 at work` for several -; where the
    line is too short, one count, `+3 at work elsewhere`.
*   **D3 - How much.** Only who is at work; what they stream stays the thread followed's.
*   **D4 - What waits.** The mark of 594 (D4) is taken along: a channel not talked in where an
    approval or questions wait for the user is marked so, `[side] waits for you`.
*   **D5 - `line`.** Tells nothing of it: a screen reader reads every line, and what does not
    concern the user is not printed there (594).

## Plan

One step. `Followed` keeps the answers under way of every thread of its channel, each with its
thread (`UnderWay::thread`), what they stream only of the thread followed, ended by their turn;
`Channels` gives the screen what is at work (`Activity`): the answers of the thread followed, and
per other channel who is at work and whether something waits for the user (`Elsewhere`). The
status line shows it; `line` shows nothing of it.

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

**2026-11-25** - What exists checked (*What exists*): `line` tells nothing either since 594,
other threads are left out too, and where an agent stands is lost in a thread followed. D1 to D5
decided by the maintainer, as recommended, the mark of 594 taken along (*Decision*). Planned in
one step (*Plan*).

**2026-11-25** - Done as planned: `Followed` keeps the answers under way of every thread
(`UnderWay::thread`, `keep_under_way`), each ended by its turn; where an agent stands is that of
its answer in the thread of the turn it runs, where it says it, or, said in the line outside a
turn, that of all its answers - the line's no longer lost where a thread is followed.
`Channels::activity` gives the screen what is at work (`Activity`, `Elsewhere`), an agent at work
in several threads counted once; the status line marks the other channels or sums them up, `line`
shows nothing of it. Following a thread reads the answers under way no more: they are kept of
every thread. Documented in README.md.

**2026-11-27** - Issue 615 (D2) moved the marks of the other places from the status line into a
footer below the line typed, which lists every channel and every thread of the channel talked in;
the status line keeps the answers of the thread followed.
