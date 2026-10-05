# 231 — The panels of the web UI have a fixed width

**Status:** open
**Area:** User interaction

## The problem

Every panel of the web UI has a width set in its class and cannot be changed: the sidebar with the
sessions and channels (`Sidebar.tsx`, `w-64`), the members of a channel (`ChatView.tsx`, `w-64`),
and the panels for what an agent heard and said and for its tools (`ContextPanel.tsx`,
`HistoryPanel.tsx`, `w-[28rem]`). Long session, channel and member names are cut off
(see [230](230-member-actions-menu.md)), and a context with long tool results is read in a narrow
column, while on a wide screen the chat takes all the room that is left.

## Why it matters

What fits depends on the screen and on what someone is doing at the moment: reading an agent's
context needs a wide panel, following the chat a narrow one. A fixed width suits neither.

## Considerations

- **A handle on the panel's inner edge,** dragged with the mouse (pointer events, so touch works
  too); the cursor shows `col-resize` over it. A double click returns to the default width.
- **Bounds:** a smallest width at which the panel is still usable, and a largest that leaves the
  chat room - relative to the window, so that a panel never covers it entirely.
- **One mechanism for all panels:** a small hook or component (width, bounds, the side of the
  handle) used by the four panels, rather than four copies of the dragging logic.
- **Remembered per viewer** in `localStorage`, per kind of panel, so that the width survives a
  reload. It is a convenience of this browser, not the user's setting: read and written in
  `try/catch`, with the default width where nothing is stored. Whether it belongs in the user's
  profile instead, so that it follows them to other devices, is to be decided - probably not, since
  the right width depends on the screen.
- **Keyboard:** the handle is focusable and changes the width with the arrow keys, with
  `role="separator"` and `aria-valuenow`.
- On narrow screens the members panel is hidden today (`hidden md:block`); that stays.

## Done when

The sidebar, the members panel and the context and history panels can be made wider or narrower with
the mouse and the keyboard, within bounds, the width is kept across reloads in the same browser, one
shared mechanism does it for all of them, and WEB-UI.md describes it.

## History

- **2026-10-05** — Issue created at the maintainer's request, together with
  [230](230-member-actions-menu.md): the width of the panels should be changeable with the mouse.
