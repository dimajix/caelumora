# 230 — The actions on a channel member crowd out its name

**Status:** open
**Area:** User interaction

## The problem

Every member of a channel in the right-hand panel of the web UI (`ChatView.tsx`) carries up to four
action buttons in its row: what it heard and said (`History`), its tools (`Wrench`), deleting the
agent (`Trash2`, with "Delete agent?" to confirm) and removing it from the channel (`UserMinus`).
They are hidden until the row is hovered (`opacity-0`), but they keep their place, so the name and
the `@handle` are cut off (`truncate`) even when no button is shown. In the narrow panel little of
the name is left - and the name is what the row is for.

The sidebar already solves the same problem for sessions and channels: one button with three
vertical dots (`MoreVertical`) opens a menu with the actions (Rename, Privacy, Delete).

## Why it matters

Members are told apart by their names; with several agents of similar names, or a long handle, the
panel no longer says who is in the channel. Every further action on a member (issue
[227](227-agent-management.md) will bring some) makes it worse.

## Considerations

- **A menu per member, like the sidebar's.** One `MoreVertical` button in the row; the menu holds
  the actions the member offers (`can_read_context`, `can_read_tools`, `can_delete`,
  `channel.can_change`), with icon and text. No button where the member offers no action. The
  former members get the same menu, with what they offer.
- **One menu component, not a second copy.** The sidebar builds its menus inline - open state,
  closing on a click outside, position, entries. A small shared component (button, entries with
  icon, label, danger style) used by the sidebar and the member list keeps the two alike; the
  sidebar would be moved to it in the same step or before.
- **Confirming the deletion of an agent** happens today in the row itself ("Delete agent?"). In the
  menu it becomes either a second click on the entry, or the entry's text changes - as long as it
  stays clear that the agent leaves every channel, not only this one
  ([WEB-UI.md](../docs/WEB-UI.md)).
- **Keyboard and screen readers:** the button needs an `aria-label` naming the member, the menu
  `role="menu"` and closing with Escape - the sidebar's menus have neither yet.
- The token gauge below the name stays where it is: it is information, not an action.

## Done when

A member's row shows its name and handle over the full width but for one menu button, its actions
are reached through that menu, the sidebar and the member list use the same menu component, and
WEB-UI.md describes it.

## History

- **2026-10-05** — Issue created at the maintainer's request: the action icons on the channel
  members leave hardly any room for the name; a menu like the one on the channel names in the
  sidebar is preferred.
