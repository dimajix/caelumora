# 40 — A new session always uses whichever template happens to come first

**Status:** done
**Area:** User interaction

## The problem

The web UI's "New Session" button creates a session from `templates[0]` (`Sidebar.tsx`,
`handleCreateSession`) - the first of whatever `GET /templates` returns. That list has no order: the
JSON store reads a directory, whose order is the file system's. Which template a new session gets is
therefore an accident; after the local store was recreated it switched from "sassy-session" to
"simple-session" without anything in the code changing. The user cannot choose.

The REST API already takes the template (`template_id` in `POST /sessions`); only the web UI offers
no way to pick one.

## Why it matters

Templates are what distinguishes one kind of session from another - which agents take part, with
which personalities, in which channels. A session created from the wrong template has to be deleted
and created again, and there is no way in the UI to get the other one at all.

## Considerations

- **Choosing in the UI:** the "+" opens a small menu listing the templates by name, with their
  description; choosing one creates the session. With a single template, "+" can create it directly
  as today.
- **A stable order:** whatever the UI shows or picks by default should not depend on the store's
  listing order. Sort by name - in the UI, or let `GET /templates` answer sorted, which every client
  would benefit from.
- **A default, if any:** remembering the last template chosen (per browser) would keep "+" a single
  click for the common case; a server-side default would need a notion of it in the templates, which
  nothing asks for yet.
- **Naming:** the new session is called "New <template name>" today; with a choice that stays
  sensible.

## Done when

- The web UI lets the user choose the template of a new session.
- Which template is offered first, or used by default, does not depend on the store's listing order.

## History

- **2026-09-25** — Opened after the local store was recreated and "New Session" silently switched
  from the sassy to the simple template: it takes `templates[0]` of an unordered list.
- **2026-09-25** — Done, along the maintainer's design rather than the menu on "+" proposed above:
  - **A dropdown with all templates, "+" next to it** creates a session from the selected one (named
    "New <template>" as before). The template's description is the dropdown's tooltip.
  - **The template last used is remembered** in the browser's local storage
    (`loadLastTemplate`/`saveLastTemplate` in `session.ts`, next to the drafts) when a session is
    created from it, and preselected next time; if it no longer exists, the first template is.
  - **It is shown in bold in the dropdown** - where the browser styles options at all; macOS does
    not.
  - **A stable order:** `GET /templates` answers sorted by name, so every client sees the same order
    rather than the store's directory order.
  - Verified in the browser: the order, creating a session from the template that was not
    preselected, and that one being preselected and bold after a reload.

