# 129 — AGENTS.md and ARCHITECTURE.md hold everything in two files

**Status:** in progress
**Area:** Maintenance

## The problem

`ARCHITECTURE.md` had grown to 360 lines of about 80 KB, most of them single
paragraphs of several hundred words. It described every subsystem in full -
components, tools, the agent turn, notes, channels, storage, security,
confidentiality, logging, errors - so it no longer drew an overall picture,
and finding the part about one subject meant reading around the others.
`AGENTS.md` mixed the rules for every change with the rules for issues, the
coding style and security-specific guidance. Logging and error handling were
described in both files. Configuration was spread over the whole of
`ARCHITECTURE.md` with no overview.

## Why it matters

- A document nobody can read in one sitting is not read, and then not kept
  up to date ([110](110-docs-contradicting-code.md)).
- `AGENTS.md` is loaded into every agent's context; what is in it should
  be what applies to every change.

## Considerations

- **One document per subject in `docs/`**, `ARCHITECTURE.md` among them as
  the overview. Chosen with the maintainer: `ARCHITECTURE.md` stays as an
  overview that draws the whole picture - more than a list of links - and
  the details move to `COMPONENTS`, `TOOLS-AND-COMMANDS`, `AGENT-TURN`,
  `MEMORY`, `CHANNELS`, `EVENTS-AND-STREAMS`, `STORAGE`, `SECURITY`,
  `CONFIDENTIALITY`, `CONFIGURATION` and `CODING-GUIDELINES`.
- **What stays in `AGENTS.md`.** Moved documents are only read when an
  agent follows the link. `AGENTS.md` therefore keeps the hard rules as a
  short list, the architecture-decision rules, and a table saying which
  document to read for which work.
- **Issue rules** move to `issues/INDEX.md`, which already described most
  of them under *How this is maintained*; a separate `issues/README.md`
  would have been a third copy.
- **References in issue files** to `ARCHITECTURE.md` are left as they are:
  they are records of when they were written.
- **Two steps**: moving the text unchanged first, so that the move can be
  checked line by line, and rewriting it afterwards.

## Done when

- `docs/` holds the documents above, `ARCHITECTURE.md` reads as an overview.
- `AGENTS.md` holds only what applies to every change, and links the rest.
- The long paragraphs are rewritten so that they can be read.

## History

- **2026-09-30** — Opened with the maintainer. First step: the text moved
  unchanged into `docs/`; `AGENTS.md` rewritten as an entry point; the rules
  for issues moved into `issues/INDEX.md`; `docs/CONFIGURATION.md` and the
  section on the model's configuration are new. Found on the way:
  [130](130-llm-configuration.md).
