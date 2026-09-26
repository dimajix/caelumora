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
| [01](01-improve-logging.md) | done | Two logging frontends (`log` and `tracing`) are used side by side, and the REST handlers, session loop and event bus are nearly silent. |
| [02](02-better-error-messages.md) | done | Failures lose the context that would make them actionable — which component, which endpoint, which parameter. |

## Persistence

| No. | Status | Issue |
|---|---|---|
| [03](03-postgres-entity-store.md) | open | `EntityStore` has only in-memory and JSON-file implementations; a Postgres one is the prerequisite for everything multi-process. |
| [04](04-postgres-state-store.md) | open | `StateStore` likewise — and it holds the state whose loss is most expensive, including agents suspended on an approval. |
| [05](05-postgres-event-recorder.md) | open | The `Recorder` hook is wired into every channel, but `BlackholeRecorder` is the only implementation, so nothing is recorded. |
| [06](06-context-in-message-for-auditability.md) | done | For a past message there is no way to reconstruct what the agent actually saw when it wrote it. |

## Sessions and channels

| No. | Status | Issue |
|---|---|---|
| [07](07-session-deletion-audit.md) | in progress | Deletion works since `540e482`; what remains open is whether and how a deletion is recorded. |
| [08](08-retry-and-edit-past-answers.md) | open | A conversation is append-only — no retry, no correcting a past message, so a derailed turn keeps influencing every later one. |
| [09](09-creating-channels.md) | done | Channels exist only as instantiated from a session template; there is no create, no close, and no way to add a participant afterwards. |
| [46](46-channel-participants.md) | open | The participants of a channel are fixed when it is built; nobody can be added to or removed from a running channel. |
| [34](34-snapshot-on-stream-connect.md) | done | Event streams deliver only what happens after they connect, so a client cannot learn the current state and loses whatever happens while it connects. |
| [38](38-global-stream-loses-events-silently.md) | done | A client of the global event stream that falls behind loses events without being told, and its session list stays wrong until a reload. |
| [39](39-global-events-not-filtered.md) | done | The global event stream sends every user the events of every session, including the ids and new names of sessions they may not see. |
| [30](30-user-actor-for-authenticated-user.md) | done | Messages and approvals from the REST API go through the channel's first `UserActor`, so they are recorded in the name of someone other than the authenticated sender. |
| [42](42-export-import-chats.md) | open | Chats cannot be exported or imported, so a conversation cannot leave the server it happened on or be brought into another. |

## Identity and access management

| No. | Status | Issue |
|---|---|---|
| [10](10-user-authentication.md) | in progress | There is no authentication at all; the REST server fabricates its own `SecurityContext` and `DummySecurityManager` approves everything. |
| [11](11-agent-ownership.md) | done | `SecurityAttributes` carries an `owner_id` that nothing enforces — every agent, session and template is effectively global. |
| [12](12-user-and-agent-permissions.md) | open | When an agent acts for a user, two sets of rights are involved and there is no rule for combining them. |
| [23](23-acls-in-security-attributes.md) | open | `SecurityAttributes` holds only an unused `owner_id` — there is no way to grant an actor or a role any right on any entity. |
| [24](24-opa-security-manager.md) | open | `DummySecurityManager` is the only implementation; deployment-specific policy has no home outside the compiled binary. |
| [25](25-file-user-backend.md) | in progress | There is no source of identities to authenticate against; a hand-edited user file is the smallest one, and defines the provider seam. |
| [26](26-database-user-backend.md) | open | A file cannot serve several processes or an existing corporate user table; no database-backed user provider exists. |
| [27](27-ldap-user-backend.md) | open | Where identities are managed in a directory, the system has no way to authenticate against it or read its groups. |
| [31](31-session-renewal-from-ui.md) | done | The session token is only renewed by requests, so a web UI that is open but only receiving events is logged out after 15 minutes and loses the typed input. |
| [33](33-sse-outlives-its-token.md) | done | An open server-sent event stream is authenticated once, when it connects, and keeps delivering after its token has expired or its user was removed. |

## Tools and approvals

| No. | Status | Issue |
|---|---|---|
| [13](13-approval-modes.md) | open | Approvals cannot be remembered — no "always approve" for a scope, no brave mode — so repeated prompts train the user to approve without reading. |
| [14](14-approval-privilege.md) | open | Any user in the channel can approve any tool call; ineligible responses cannot be ignored because eligibility does not exist. |
| [15](15-disallowed-tool-errors-to-llm.md) | done | A refused tool call does not reach the model as something it can act on, so the agent retries or proceeds as if it had succeeded. |
| [32](32-unavailable-tools-invisible-to-model.md) | open | A tool source that failed to load (e.g. an unreachable MCP server) leaves no trace in the model's context, so the agent acts as if the tools never existed. |
| [49](49-plan-step-system-messages.md) | open | Plan step instructions are system messages in the middle of a turn, which some models move into the system prompt, losing their place and the prompt cache. |
| [52](52-split-tool-providers.md) | done | All built-in tool providers live in one file, and tools added later went to other places, so there is no rule for where a tool goes. |
| [37](37-tool-name-collisions.md) | done | When two components of an agent provide a tool of the same name, the first one silently wins and the other is unreachable. |

## User interaction

| No. | Status | Issue |
|---|---|---|
| [28](28-user-commands.md) | in progress | Everything a user types goes to the agents; there is no way to address the system itself with slash commands such as `/help`, `/wakeup` or `/compact`. |
| [35](35-sidebar-live-updates.md) | done | The web UI's session list is loaded once and never updated, so sessions and channels created or deleted elsewhere stay invisible until a reload. |
| [40](40-choose-session-template.md) | done | A new session in the web UI always uses the first template of an unordered list, and the user cannot choose another. |
| [41](41-show-message-context.md) | done | The web UI offers no way to see what an agent's model saw when it produced a message, although the server can answer it. |
| [43](43-message-time-and-statistics.md) | done | The web UI shows neither when a message was written nor the token counts and durations recorded for it. |
| [29](29-mentions.md) | done | In a multi-agent channel the user cannot choose who answers; the moderator alone decides, and an `@name` in the text means nothing to it. |
| [44](44-mentions-only-moderator.md) | done | Every moderator makes some agent answer every message; there is none under which agents respond only when they are `@`-mentioned. |
| [45](45-agent-mentions.md) | open | An `@`-mention written by an agent does not make the mentioned agent respond, so agents cannot hand questions to each other. |

## Context and knowledge

| No. | Status | Issue |
|---|---|---|
| [16](16-context-compression.md) | open | History and context grow without bound; nothing decides what to drop before the model's context window is exceeded. |
| [17](17-information-sharing-within-session.md) | done | Agents in one session share nothing but routed messages — there is no scratchpad one can write to and another read. |
| [18](18-knowledge-sharing.md) | open | Sessions are sealed: nothing an agent learns outlives the session, and cross-agent sharing is a harder question again. |
| [19](19-memory-tiers.md) | open | The four memory tiers from `TODO.md` are named but undefined — scope, lifetime, readers and invalidation are all open. |
| [47](47-prompt-parts-with-provenance.md) | done | What a model is given besides the conversation is placed and recorded on two separate paths, and can only be a system prompt, whose every change invalidates the prompt cache. |
| [48](48-shared-digest-chain.md) | done | History and context duplicate one digest-chained structure, and notes would become a third copy that branching would have to change separately. |
| [50](50-turn-context-naming.md) | open | `TurnContext` calls both the stored context and the input of one model call "context". |
| [51](51-agent-private-notes.md) | done | An agent knows in each channel only what was said there; it has no private knowledge that carries over to its other channels. |
| [54](54-private-notes-visible-to-users.md) | open | Private notes meant for the agent alone are visible to users, in the context panel and as the tool call in the chat. |
| [55](55-notes-by-reflection.md) | open | Notes are written only when the model thinks of calling a note tool while answering, so what an agent learnt in a turn is kept unreliably and never tidied up. |
| [22](22-skill-management.md) | open | Skills do not exist: there is no unit of procedural knowledge below a personality, and no rule for who may load one into a turn. |

## Maintenance

| No. | Status | Issue |
|---|---|---|
| [36](36-replace-serde-yaml.md) | done |
| [53](53-facet-trait-placement.md) | done | The traits components implement as facets live in different places, some among the implementations, and `MessageProcessor` names two unrelated traits. | The configuration is parsed with `serde_yaml`, which is deprecated and will receive no further fixes. |

## Open architecture questions

| No. | Status | Issue |
|---|---|---|
| [20](20-transactional-effects.md) | open | A cognitive turn persists state, runs tools and emits events with no boundary around them; a crash mid-tool re-executes it on restart. |
| [21](21-scaling.md) | open | The runtime is single-process by design; which of the three scaling axes actually matters has never been decided. |
