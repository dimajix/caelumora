# 536 — The cards of a message are handed what they know of the channel prop by prop

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** `MessageView` takes eight props of the channel only to hand them on to `ToolCallView`, which hands most of them on to the handler of a presentation, so that whatever a card needs to know of the channel adds a prop to three components.

## The problem

What the card of a tool call shows of the channel - the workspace's files and when they changed,
the artifacts, the attachments, the commands ended, the digests of what each turn carries (issue
[535](535-tool-media-attached-kept-and-shown-twice.md)), and the actions of opening a page and
ending a command - is computed by `ChatView` and reaches the card through two components that only
pass it on: `MessageView` takes `files`, `filesVersion`, `artifacts`, `attachments`, `onSelectPage`,
`onKillCommand`, `endedCommands` and `attachedInTurn` and uses only `attachments` itself;
`ToolCallView` takes them again and hands six of them on to the handler of the call's presentation
(`ToolCallContentProps`), most of which use none of them.

Each new thing a card needs of the channel is one more prop in three components, and one more place
for `ChatView`, the context panel and the history panel to remember to pass it - or to leave it out,
as the panels do on purpose, without that being said anywhere.

## Considerations

- **A. A React context of the channel the messages are shown in.** `ChatView` provides once what
  the messages it shows need of their channel; `MessageView`, `ToolCallView` and the handlers read
  it where they use it. A card is given what is its own - its call, its result, its message's id
  and turn -, nothing it only passes on. The panels provide one of their own, with the attachments
  alone, which says that their cards show the record whole. The web UI uses no context yet; this
  would be its first, for exactly what a context is for: what a subtree shares, read deep below.
- **B. One object instead of the eight props.** A `channel` prop bundling them is passed down as
  before. Fewer props, but still passed through every level, and one more type that only bundles
  parameters.
- **C. As it is.** Explicit, but the list grows with every card that learns something new.

A is chosen: it removes the passing on rather than packing it.

## Done when

- `ChatView`, `ContextPanel` and `HistoryPanel` provide what their messages need of the channel in
  one place; `MessageView` and `ToolCallView` take no prop they only hand on, and the handlers of
  the presentations take their call and result.
- The chat and the panels show what they showed before.
- WEB-UI.md names the context and what the panels leave out of it.

## History

- **2026-11-03** — Issue created, from the review of issue
  [535](535-tool-media-attached-kept-and-shown-twice.md), which added the eighth such prop.
