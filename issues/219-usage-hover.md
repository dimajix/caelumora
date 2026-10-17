# 219 — The usage statistics on hover look makeshift

**Status:** done
**Area:** User interaction
**Summary:** The token usage is shown in two hovers built differently - a box too narrow for lines that wrap, and the browser's plain tooltip - and neither looks good.

## The problem

The web UI shows token usage in two hovers, built in two different ways, and neither looks good:

- **A message's statistics** (`UsageInfo` in `MessageView.tsx`, issues 43 and 209): an "(i)" that
  opens a box of a fixed width (`w-56`) with label/value rows. When a row is longer than that -
  a value such as "8 % of 65,536", or one of the longer labels - it wraps, and the box is then too
  small for its content: label and value no longer line up, and the rows look crowded.
- **An agent's gauge and the channel's total** (`TokenGauge.tsx`, the "Total tokens" line in
  `ChatView.tsx`, issue 209): the browser's own tooltip (`title`), fed with lines of plain text
  (`tokenUsageText`). It cannot be styled, appears only after a delay, not on touch, and its
  groups - "Context of the last call", "Usage of all N calls" - are told apart only by an empty
  line.

## Why it matters

- The usage is what issue 209 was about; shown cramped or as plain text, it is harder to read than
  it needs to be.
- Two ways of showing the same kind of numbers look different for no reason.

## Considerations

- **One component for both:** a hover card that sizes itself to its content (a minimum width, no
  fixed one, values not wrapping), with the groups as headed sections and the numbers aligned
  right. `UsageInfo` already opens on hover, focus and tap; the gauge and the total would use the
  same.
- **Placement:** the box opens above its trigger and to the right; at the edge of the chat or of
  the narrow member list it may be cut off. It should stay inside the window.
- **Content:** the gauge's card could show the bar again, larger, with the context size; the
  message's card the same groups as the agent's where they apply (context of this call, its input
  and output tokens, durations).
- A library for floating elements would solve the placement, but adds a dependency for one thing;
  plain CSS may do if the card opens toward the free side.

## Done when

- A message's statistics, an agent's gauge and the channel's total show their numbers in one
  styled hover card that fits its content, lines up labels and values, and stays inside the window.
- WEB-UI.md says so.

## History

- **2026-10-04** — Issue created at the maintainer's request: the hover is sometimes too small when
  a line wraps, and does not look good otherwise.
- **2026-10-04** — Done. One `HoverCard` for all three, with the numbers as `UsageStats`: groups
  under a heading ("Context of the last call" and "Usage of all N calls" for an agent, "Tokens"
  and "Time" for a message), labels left, values right in tabular figures, nothing wrapped, the
  card as wide as its content. It opens on hover, focus and tap, above its trigger or below where
  there is no room, kept inside the window; it is placed in the page's body, so that the scrolling
  member list does not cut it off, and closes when the page scrolls. No library was needed.
  `tokenUsageText` and `usageRows` became `tokenUsageGroups` and `usageGroups`. Checked in
  headless Chromium against a running server, light and dark: all three cards fit their content,
  the channel's total is moved left to stay inside the window, and a message at the top edge opens
  its card below.
