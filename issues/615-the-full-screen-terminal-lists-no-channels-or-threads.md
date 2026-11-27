# 615 — The full-screen terminal lists no channels or threads

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** `caelumora-tui`'s full-screen view shows nothing below the line typed and has a status line only while an answer is under way, so whoever works there does not see which channels the session has, which threads the channel talked in has, where they are, or anything else of the session at a glance, as Claude Code's status line shows it.

## The problem

The full-screen view (issue 569) is laid out from top to bottom as: what was printed, what is
asked of the user, a status line, the candidates of a completion, a rule, the line typed
(`caelumora-tui/src/screen/full/mod.rs`, `Layout::vertical`). Nothing stands below the line typed.

The status line is drawn only while something is at work (`status_rows = u16::from(at_work)`): the
answers under way in the thread followed, and, since issue
[572](572-the-full-screen-terminal-tells-nothing-of-other-channels-at-work.md) (D2, D4), a short
mark per other channel where an agent is at work or something waits for the user. When nothing is
at work, the window says nothing of where the user is:

- which channels the session has, and which one they talk in;
- which threads the channel has - every sub-agent works in one (issue 558) - and which one they
  follow;
- where something is new, at work or waits for them, when no agent happens to be at work there.

To find out, the user types `/channel list` or `/thread list`, and switches with
`/channel switch` and `/thread follow` by a name or number they first have to look up.

## Why it matters

- With sub-agents, a channel has threads that come and go as agents start them; without a list
  they are invisible until something in them concerns the user.
- Claude Code shows below its input a status line that is always there and says where one is; the
  full-screen view was meant to feel like it (README.md, issue 569).

## Considerations

### What the line shows

A footer below the line typed, always there in `full`, one row - two where the window is wide
enough and there is that much to show:

- **The channels of the session**, the one talked in marked, each with what 572 and 594 mark: an
  agent at work, something waiting for the user, something new not yet seen.
- **The threads of the channel talked in**, by the name of their founder - the sub-agent's name, as
  `/thread list` names them -, the one followed marked, with the same marks; the line itself
  first.
- **Further information**, to be chosen: the session's name, the agent talked to and how full its
  context is (its model's context size, as compaction measures it), what the turns cost so far
  (`/agent usage`), the workspace and its directory. Claude Code shows model, directory and context
  use; which of these matter here is the maintainer's choice.

### The marks of 572

572 put its marks into the status line, after the answers under way. With a footer, they belong
there: the status line keeps the answers of the thread followed - what they stream and where they
stand -, the footer says where the user is and what goes on elsewhere. One place per kind of
information, so that 572's marks are moved, not shown twice. That changes a decision of 572 (D2)
and needs the maintainer's word.

### Room

- A narrow window cannot show every channel: the one talked in always, then those with a mark, then
  the rest as long as room is left, and `+3` for what is not shown - as 572 sums up.
- A short window loses a row of the printed text to the footer; below some height the footer is
  left out.

### Switching from it

The footer could number its entries, so that a key - `Alt+1` … `Alt+9`, or a click on an entry -
switches channel or thread without a command. It can come in a second step; the first is to show.

### `line`

The line-wise view prints nothing of it: a screen reader reads every line, and what does not
concern the user is not printed there (572, D5; 594). `/channel list` and `/thread list` stay its
way.

### Not this issue

- Colours of the status line and of everything else: issue
  [578](578-the-full-screen-terminal-is-drawn-in-few-colours.md).
- The web UI, which lists channels and threads in its own way (WEB-UI.md).

## Done when

- The full-screen view shows below the line typed the channels of the session and the threads of the
  channel talked in, which of them the user is in, and where something is at work, new or waits.
- What further information it shows is decided and shown.
- Where 572's marks stand is decided; nothing is shown twice.
- Tested as the window is tested (`the_window_shows_…`); documented in README.md.

## What exists

Checked against the code on 2026-11-27:

*   **"New" has no source in the terminal.** The kernel counts what is new to a user per channel
    and per thread (`Unread`, `Detail::UNREAD`, `ChannelView::unread`), from what they marked
    read (`UserController::mark_read`), as the web UI does. `caelumora-tui` marks nothing read: to
    whoever works in the terminal alone everything stays new, and the web UI counts as new what
    they read there.
*   **The threads are read, not kept.** `Followed::open_threads` reads them from the channel's
    snapshot and the message each hangs at, asynchronously; what the screen is given
    (`Channels::activity`) is made synchronously after everything that happens
    (`main.rs`). `ActorEvent::ThreadOpened` carries the thread and its origin, not whom it was
    delegated to; whether that is known as it opens is not yet checked.
*   **`Elsewhere` is a summary.** It lists only the channels with a mark, and the other threads
    of the channel talked in as one entry of that channel.
*   **Further information.** `ParticipantView::usage` (`TokenUsage`) and
    `ParticipantView::context` exist; how full a context is, as compaction measures it, is not
    yet checked. "The agent talked to" is no one agent where a channel has several.

## Decision

Decided by the maintainer on 2026-11-27, as recommended, with a rule below the line typed added:

*   **D1 - What the footer shows.** Below the line typed, set apart from it by a rule as the line
    is from what was printed: in its first row the channels followed, then the line and the open
    threads of the channel talked in, each with where the user is and its marks; in a second row,
    where the window is wide enough, the further information (D4).
*   **D2 - 572's marks move.** They stand in the footer, not in the status line, which keeps the
    answers under way of the thread followed alone. This changes 572 (D2).
*   **D3 - "New" later.** The first steps show no mark of what is new. Where it comes, it comes
    from the kernel's `Unread`, the terminal marking read what it prints of the thread followed
    (`UserController::mark_read`) - not from a count of the terminal's own, which would be a
    second concept beside `Unread`.
*   **D4 - Further information.** The session's name; how full the contexts of the agents of the
    channel talked in are, once it is checked how it can be read. Neither cost nor directory:
    the directory is the one the program was started in, whose session it is.
*   **D5 - Room.** The channel talked in and the thread followed always, then the places with a
    mark, then the rest as long as room is left, and `+3` for what is not shown. Below 10 rows of
    the window the footer and its rule are left out.
*   **D6 - Switching from it.** By `Alt+1` … `Alt+9` or a click, in a later step.
*   **D7 - Colour.** The footer brings its colours now, apart from issue 578, and only with the
    meanings `styled.rs` gives them already: where the user is bold, without brackets; a place
    where nothing goes on dimmed; an agent at work there marked by a cyan `●`, as the spinner of
    an answer is cyan; something waiting for the user by a yellow `!`, as approvals and "waits for
    you" are yellow; the separator, `+3`, the session's name and the rule dimmed. Colour never
    carries a meaning alone - the mark, the bold say it too -, so that a terminal without colour,
    or `NO_COLOR` once 578 brings it, loses nothing. The terminal's sixteen colours, which follow
    its theme; no background of its own; nothing in the footer moves. Drawn in `styled.rs`, as the
    status line is.

What the screen is given (`Activity`) names every place the user can be, `Place` taking
`Elsewhere`'s:

```rust
pub(crate) struct Activity {
    pub(crate) here: Vec<UnderWay>,
    /// The channels followed, in the order of the session.
    pub(crate) channels: Vec<Place>,
    /// The line and the open threads of the channel talked in, the line first.
    pub(crate) threads: Vec<Place>,
}

/// A place the user can be - a channel followed, a thread of the channel talked in - and what
/// goes on there.
pub(crate) struct Place {
    pub(crate) name: String,
    pub(crate) current: bool,
    pub(crate) at_work: Vec<String>,
    pub(crate) waits: bool,
}
```

`Followed` keeps the threads open in its channel (`Followed::threads`, `&[OpenThread]`), each with
whom it was delegated to; `OpenThread` loses `told`, which `/thread list` makes of it and of the
message it hangs at.

## Plan

Each step a commit of its own, reviewed before the next.

1.  **The threads kept.** `Followed` keeps its open threads, read as it is followed or catches up
    and kept up as threads open and close; `/thread list` built on them. No change of behaviour.
    First checked: how whom a new thread was delegated to becomes known - read anew at
    `ThreadOpened`, or carried by the event, which changes the domain and is put before the
    maintainer.
2.  **The footer.** `Place` instead of `Elsewhere`, the footer with its rule below the line typed,
    in its colours (D7), 572's marks moved there from the status line, the session's name; tested
    as the window is, documented in README.md.
3.  **Later, each decided anew:** what is new (D3), how full the contexts are (D4), switching
    from the footer (D6).

## History

- **2026-11-27** — Opened on the maintainer's wish for a status line as Claude Code has, listing
  the channels and threads below the line typed.
- **2026-11-27** — What exists checked (*What exists*): no source of what is new in the terminal,
  the threads read and not kept, `Elsewhere` a summary. D1 to D7 decided by the maintainer, as
  recommended, with a rule between the line typed and the footer, and the footer's colours taken
  from issue 578 (*Decision*). Planned in steps (*Plan*).
- **2026-11-27** — Step 1 checked first: whom a thread was delegated to is not known as it opens
  (`ActorChange::Open` records no delegation); `ActorChange::Delegate` sets it later, and the
  kernel announces no delegation of its own - the first brief the thread's delegate gives
  (`GivenToRead`, sent by `ActorId::Delegate` of that thread) is the first that tells it. Step 1
  done, without a change of the domain: `Followed` keeps the threads open
  (`Followed::threads`), read as it is followed or catches up, and read anew in `take_in` where
  a thread opens or closes or its delegate gives its first brief; `/thread list`, `/thread
  follow` and the notice of talking in a channel use what it keeps. Whether the kernel is to
  announce a delegation, for this terminal and the web UI alike, is put before the maintainer.
- **2026-11-27** — Step 2 done: `Activity` names the session, the answers of the thread followed,
  the channels followed and the line and the threads open of the channel talked in, each a
  `Place` (`Elsewhere` gone); `Channels::activity` marks every place but where the user is. The
  full-screen view draws below the line typed a rule and the footer (`footer_rows`): the places,
  fitted as D5 says, in the colours of D7, and the session's name below; neither in a window
  lower than `FOOTER_HEIGHT` rows, or where what is asked needs the room. The status line shows
  the answers of the thread followed alone. Unlike the interface sketched in *Decision*,
  `Place::at_work` is a flag, since the footer names no agent at work, and `Activity` carries the
  session's name (D4); unlike D1, the session's row stands whatever the width. Documented in
  README.md.
- **2026-11-27** — Whether the kernel announces a delegation, for the terminal and the web UI
  alike, opened as issue [621](621-a-delegation-is-announced-to-nobody.md).
