# 132 — The REST server and the web UI have no document of their own

**Status:** done
**Area:** Maintenance

## The problem

What the REST server does is spread over the documents of other subjects: the token handling (login,
cookies, renewal, bounding a token) is in `docs/SECURITY.md`, the server-sent event streams
(snapshots, resync, token expiry, `require_stream_auth`) in `docs/EVENTS-AND-STREAMS.md`, the
problem details in `docs/CODING-GUIDELINES.md`, and endpoints appear wherever the operation behind
them is described. The web UI has no document at all; what it does - how it follows the streams,
renews its session, recognises commands, shows notes and an agent's context - is scattered in half a
sentence here and there across most documents in `docs/`.

## Why it matters

- Whoever works on the REST server or the web UI has to collect what applies from every other
  document, and AGENTS.md asks for REST changes to stay compatible with the web UI without saying
  where that contract is described.
- The runtime documents are cluttered with frontend details that do not concern the subject they
  describe.

## Considerations

- **`docs/REST-SERVER.md`**: authentication and the token (from `SECURITY.md`, which keeps users,
  the user providers and the policy), the streams (from `EVENTS-AND-STREAMS.md`, which keeps the
  event bus and could then be called `EVENTS.md`), how errors are answered (problem details,
  `ApiError`), how responses are mapped from views, and an overview of the endpoints.
- **`docs/WEB-UI.md`**: its structure, how it follows the global and channel streams
  (`openEventStream`), keeps its session alive, mirrors `parse_input` in `command.ts`, and what it
  shows (members, notes, context panel, moderator); the runtime documents keep only a link.
- Both documents follow the layout of the others (intro with links, short paragraphs, 78 columns)
  and get a line in the table of AGENTS.md and the list in `docs/ARCHITECTURE.md`.

## Done when

- `docs/REST-SERVER.md` and `docs/WEB-UI.md` exist, and the other documents link to them instead of
  describing the frontends themselves.

## History

- **2026-09-30** — Opened at the maintainer's request, following the restructuring in
  [129](129-documentation-structure.md).
- **2026-09-30** — Done. `docs/REST-SERVER.md` holds the server's structure, how a handler works,
  the endpoints, authentication and the token (from `SECURITY.md`, which keeps a short section and a
  link), the problem details (the format from `CODING-GUIDELINES.md`, which keeps the rules for
  handlers) and the streams. `EVENTS-AND-STREAMS.md` is renamed `EVENTS.md` and keeps the event bus.
  `docs/WEB-UI.md` describes running the UI, what it decides and does not, its structure, how it
  talks to the server, and what it shows; the runtime documents keep a sentence and a link where
  they described the UI. Found on the way: renaming a channel (`ChannelLoop::update_name`) has no
  endpoint, and the web UI's own `README.md` is still the Vite template.
