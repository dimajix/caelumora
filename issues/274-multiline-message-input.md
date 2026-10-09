# 274 — The message field of the web UI has a single line that neither wraps nor takes a line break

**Status:** done
**Area:** User interaction

## The problem

The field in which a user writes a message (`ChatView.tsx`) is an `<input type="text">`. A long
message does not wrap; the line scrolls to the right, and what was written at its start is out of
sight while the user writes its end. A line break cannot be entered at all: `Enter` sends, and
there is no other key for a new line. A message with paragraphs, a list or a block of code -
exactly what a user hands an agent - can only be pasted, and even then its line breaks are lost,
since a single-line field drops them.

## Why it matters

Messages to agents are often longer than a line: a task with its conditions, a piece of text to
work on, an error message. Writing them in a field that shows a fraction of them invites mistakes,
and a message whose structure is lost is harder for the agent to read as well. Every chat a user
knows offers a field that grows with its text, and `Shift+Enter` for a new line.

## Considerations

**A `<textarea>` instead of the `<input>`.** It wraps by itself and takes line breaks. What the
field does now has to stay:

- `Enter` sends, `Shift+Enter` inserts a line break. While a completion list (`@`-mentions, `/`
  commands) is open, `Enter` keeps choosing the highlighted entry, as now.
- `Enter` during an input method composition (`e.nativeEvent.isComposing`) neither sends nor
  chooses; with a single-line field that did not matter, with a key that now sends it does.
- `ArrowUp` and `ArrowDown` choose in the completion list while it is open; otherwise they move the
  caret between lines, as a textarea does.
- The caret tracking (`onSelect`, `setSelectionRange`), the read-only state while sending and the
  combobox attributes carry over unchanged; their handlers' event types change from
  `HTMLInputElement` to `HTMLTextAreaElement`.

**Growing with the text, up to a limit.** The field starts as one line and grows with its content,
up to a maximum height (around a third of the chat's height, or a fixed number of lines); beyond
that it scrolls and shows its scroll bar. Two ways:

- CSS `field-sizing: content` with `max-height` - no code, but not supported by every browser yet
  (Firefox, Safari); where it is not, the field would stay at its initial height.
- Setting the height from `scrollHeight` on every change (reset to `auto`, then measure) - a few
  lines of code, works everywhere. The likelier choice; `field-sizing` can replace it once it is
  supported broadly.

After sending, the field shrinks back to one line. The send and stop buttons keep their place at
the bottom of the field (`items-end`) rather than being stretched with it, and the completion list
stays anchored above the field however tall it is.

**Line breaks must survive the display.** Messages are shown as Markdown (`MarkdownMessage`, with
`remark-gfm`), in which a single line break is a soft break and is shown as a space. A user who
writes two lines and sees one would think the line break got lost. Either messages of users are
rendered with hard line breaks (e.g. `remark-breaks`, for user messages only, since agents write
Markdown deliberately), or the user is expected to write Markdown and leave a blank line. The
first matches what users expect of a chat; to be decided when implemented.

**Completion over several lines.** `mentionQueryAt` looks at the text before the caret and needs
no change. `commandQueryAt` takes a command only at the start of the text; whether a command may
have arguments on further lines is not part of this issue - a line break should simply end the
command's name.

**Not part of this issue:** the terminal client (`caelumora-tui`) and the other single-line fields
of the web UI (dialogs, the answer field of a question). The free-text answer to an agent's
question (`QuestionCard`) would gain from the same field; if so, the growing textarea becomes a
component of its own, used by both.

## Done when

- A long message wraps in the field instead of scrolling sideways.
- `Shift+Enter` inserts a line break, `Enter` sends, and `Enter` still chooses in an open completion
  list.
- The field grows with its content up to a maximum height, then scrolls with a visible scroll bar,
  and returns to one line after sending.
- A message with line breaks is shown with them in the chat.

## History

- 2026-10-09: Opened, at the maintainer's request: the message field neither wraps long lines nor
  takes a manual line break, and should grow to several lines before it scrolls.
- 2026-10-09: Done. The message field is a `GrowingTextarea`, a component of its own: a textarea of
  one line that sets its height from its `scrollHeight` whenever its text or its width changes,
  capped by the `max-height` its caller gives it (a third of the window for the message field).
  `Enter` sends through the form, `Shift+Enter` breaks the line - also while the completion list is
  open, where `Enter` still chooses - and a key ending an input method's composition does neither.
  The controls beside the field keep the height of one line and sit at its bottom. Choosing a
  command no longer swallows a line break after its name. A user's message is rendered with hard
  line breaks (`remark-breaks`, new dependency), an agent's as before. `QuestionCard` keeps its
  single-line field for now.
