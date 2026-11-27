# 615 — The full-screen terminal lists no channels or threads

**Status:** open
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

## History

- **2026-11-27** — Opened on the maintainer's wish for a status line as Claude Code has, listing
  the channels and threads below the line typed.
