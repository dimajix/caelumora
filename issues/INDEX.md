# Open Issues — Index

One line of status and one sentence per issue. The reasoning, the discussion
and the decision live in the file next to it — there it may be as long as it
needs to be, here it may not.

Why the split: this index is read every time anyone looks; an issue file only
when someone works on that issue. Keeping both in one file meant either the
overview drowned in the detail, or the detail was never written down. But the
detail is the valuable part: why something is open, what already argues
against it, and what came out of the last attempt.

## How this is maintained

- **One issue is two places:** a line here, and a file
  `issues/<number>-<slug>.md`. They are created together and neither one ever
  disappears.
- **Numbers are never reused** and never reassigned. A new issue gets the next
  free number, even if issues before it are closed. Numbers are referred to —
  from commits, from code comments, from the other documents; a number that
  moves eventually points at something else. Numbers are written with **two
  digits** (`01`, not `1`) so the file listing sorts correctly; from `100`
  onwards, three.
- **When an issue is resolved, the status changes** — here *and* at the top of
  the file. Nothing else: no deleting, no rewriting of the description. What
  was once open and how it turned out is exactly the information a deleted
  line no longer gives.
- **What happened goes under "History"** at the end of the issue file, with a
  date, as a new paragraph. The existing text stays, even once it is
  outdated — it is the record of the problem, not of the solution.
- **Status:** `open` · `in progress` · `done` · `rejected`. `rejected` means:
  looked at and deliberately not done, with the reason attached — because that
  reason is otherwise worked out again from scratch six months later.

---

## Observability and operations

| No. | Status | Issue |
|---|---|---|
| [01](01-improve-logging.md) | open | Two logging frontends (`log` and `tracing`) are used side by side, and the REST handlers, session loop and event bus are nearly silent. |
| [02](02-better-error-messages.md) | open | Failures lose the context that would make them actionable — which component, which endpoint, which parameter. |

## Persistence

| No. | Status | Issue |
|---|---|---|
| [03](03-postgres-entity-store.md) | open | `EntityStore` has only in-memory and JSON-file implementations; a Postgres one is the prerequisite for everything multi-process. |
| [04](04-postgres-state-store.md) | open | `StateStore` likewise — and it holds the state whose loss is most expensive, including agents suspended on an approval. |
| [05](05-postgres-event-recorder.md) | open | The `Recorder` hook is wired into every channel, but `BlackholeRecorder` is the only implementation, so nothing is recorded. |
| [06](06-context-in-message-for-auditability.md) | open | For a past message there is no way to reconstruct what the agent actually saw when it wrote it. |

## Sessions and channels

| No. | Status | Issue |
|---|---|---|
| [07](07-session-deletion-audit.md) | in progress | Deletion works since `540e482`; what remains open is whether and how a deletion is recorded. |
| [08](08-retry-and-edit-past-answers.md) | open | A conversation is append-only — no retry, no correcting a past message, so a derailed turn keeps influencing every later one. |
| [09](09-creating-channels.md) | open | Channels exist only as instantiated from a session template; there is no create, no close, and no way to add a participant afterwards. |

## Identity and access management

| No. | Status | Issue |
|---|---|---|
| [10](10-user-authentication.md) | open | There is no authentication at all; the REST server fabricates its own `SecurityContext` and `DummySecurityManager` approves everything. |
| [11](11-agent-ownership.md) | open | `SecurityAttributes` carries an `owner_id` that nothing enforces — every agent, session and template is effectively global. |
| [12](12-user-and-agent-permissions.md) | open | When an agent acts for a user, two sets of rights are involved and there is no rule for combining them. |
| [23](23-acls-in-security-attributes.md) | open | `SecurityAttributes` holds only an unused `owner_id` — there is no way to grant an actor or a role any right on any entity. |
| [24](24-opa-security-manager.md) | open | `DummySecurityManager` is the only implementation; deployment-specific policy has no home outside the compiled binary. |
| [25](25-file-user-backend.md) | open | There is no source of identities to authenticate against; a hand-edited user file is the smallest one, and defines the provider seam. |
| [26](26-database-user-backend.md) | open | A file cannot serve several processes or an existing corporate user table; no database-backed user provider exists. |
| [27](27-ldap-user-backend.md) | open | Where identities are managed in a directory, the system has no way to authenticate against it or read its groups. |

## Tools and approvals

| No. | Status | Issue |
|---|---|---|
| [13](13-approval-modes.md) | open | Approvals cannot be remembered — no "always approve" for a scope, no brave mode — so repeated prompts train the user to approve without reading. |
| [14](14-approval-privilege.md) | open | Any user in the channel can approve any tool call; ineligible responses cannot be ignored because eligibility does not exist. |
| [15](15-disallowed-tool-errors-to-llm.md) | open | A refused tool call does not reach the model as something it can act on, so the agent retries or proceeds as if it had succeeded. |

## User interaction

| No. | Status | Issue |
|---|---|---|
| [28](28-user-commands.md) | open | Everything a user types goes to the agents; there is no way to address the system itself with slash commands such as `/help`, `/wakeup` or `/compact`. |
| [29](29-mentions.md) | open | In a multi-agent channel the user cannot choose who answers; the moderator alone decides, and an `@name` in the text means nothing to it. |

## Context and knowledge

| No. | Status | Issue |
|---|---|---|
| [16](16-context-compression.md) | open | History and context grow without bound; nothing decides what to drop before the model's context window is exceeded. |
| [17](17-information-sharing-within-session.md) | open | Agents in one session share nothing but routed messages — there is no scratchpad one can write to and another read. |
| [18](18-knowledge-sharing.md) | open | Sessions are sealed: nothing an agent learns outlives the session, and cross-agent sharing is a harder question again. |
| [19](19-memory-tiers.md) | open | The four memory tiers from `TODO.md` are named but undefined — scope, lifetime, readers and invalidation are all open. |
| [22](22-skill-management.md) | open | Skills do not exist: there is no unit of procedural knowledge below a personality, and no rule for who may load one into a turn. |

## Open architecture questions

| No. | Status | Issue |
|---|---|---|
| [20](20-transactional-effects.md) | open | A cognitive turn persists state, runs tools and emits events with no boundary around them; a crash mid-tool re-executes it on restart. |
| [21](21-scaling.md) | open | The runtime is single-process by design; which of the three scaling axes actually matters has never been decided. |
