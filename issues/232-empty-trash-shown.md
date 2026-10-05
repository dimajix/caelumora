# 232 — The trash is shown in the sidebar even where there is nothing in it

**Status:** done
**Area:** User interaction

## The problem

The sidebar of the web UI always shows a folded "Deleted sessions" below the sessions, and a folded
"Deleted channels" in every unfolded session (`Sidebar.tsx`, `DeletedList.tsx`, issue
[164](164-soft-deletion-and-former-members.md)). Mostly there is nothing in them: nothing was
deleted, or what was deleted belongs to someone else. Only unfolding one says so ("Nothing
deleted"). Every unfolded session thus carries a line without content, and the sidebar is longer
than what it shows.

## Why it matters

The sidebar is the most looked-at part of the web UI, and since 231 its width is the user's to
choose - its height is not. A line that is almost always empty costs room and attention, and it
invites a click that shows nothing.

## Considerations

- **"Nothing deleted" and "nothing the user may see" are one case for the web UI.** The server
  returns only what the user may read: `list_deleted_x` asks `ListX` and then `ReadDeleted` for each
  entry (`store_impl/access.rs`), and by the default policy only the owner reads what was deleted.
  An empty list answers both questions of the request; no permission has to be asked separately,
  and the web UI must not ask one itself.
- **What the web UI does not know today is whether a list is empty,** because `DeletedList` loads
  only when unfolded. Three ways to learn it:
  - **Load when shown, not when unfolded** (recommended). `DeletedList` loads as soon as it is
    rendered and again when `version` changes, as it does now once unfolded, and renders nothing
    while the list is empty or still loading. That is one request for the deleted sessions and one
    per *unfolded* session for its deleted channels - folded sessions render no list and load
    nothing. No change to the REST API or the runtime; `version` already changes on
    `SessionDeleted`, `SessionRestored`, `ChannelDeleted` and `ChannelRestored`, so a list that
    gains its first entry appears, and one whose last entry is restored or purged disappears.
  - **The server says it**: a count or a flag of deleted channels in the session's snapshot, of
    deleted sessions in the global snapshot. Saves the requests, but makes every snapshot ask
    `ReadDeleted` for every deleted entry, adds fields to the REST API that serve one hint in the
    web UI, and has to be kept current through the events as the lists are anyway. Only worth it if
    the requests of the first way prove too many - unlikely, since only unfolded sessions load.
  - **Hide a list after it was found empty once.** Needs no request up front, but shows every list
    until it has been unfolded once, which is the state the request complains about.
- **Purging:** after "Delete for good" or "Restore" of the last entry the list is empty and should
  disappear at once, from its own state (`act` already removes the entry), not only after the next
  event.
- **The "Nothing deleted" placeholder** goes, since an empty list is no longer shown. Whether a
  user then still finds the trash is no problem: it appears the moment something is in it.
- **Errors:** a list that fails to load stays hidden and reports the error as now, rather than
  showing an empty heading.

## Done when

"Deleted sessions" and "Deleted channels" appear in the sidebar only while they hold an entry the
user may see, appear and disappear as things are deleted, restored and purged - here or by someone
else - and WEB-UI.md says so.

## History

- **2026-10-05** — Issue created at the maintainer's request: the two trash sections of the sidebar
  should be hidden when there is nothing deleted, or nothing the logged-in user may see.
- **2026-10-05** — Done, the recommended way. `DeletedList` loads when it is rendered and again
  when `version` changes, no longer when unfolded, and renders nothing while its list is not loaded,
  failed to load or is empty; the "Loading…" and "Nothing deleted" lines went with it. Restoring or
  purging the last entry empties the list from its own state, so it disappears at once. No change to
  the REST API or the runtime. WEB-UI.md updated.
