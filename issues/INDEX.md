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
| [62](62-json-store-concurrent-writes.md) | done | Concurrent writes of the same JSON store file share one temporary file, so one fails and the newest state can be lost. |
| [77](77-template-swallows-store-errors.md) | done | `ParticipantTemplate::instantiate` treats every store error as "not found" and creates a second agent. |
| [93](93-entity-store-dialects.md) | done | The `EntityStore` offers a different set of methods for every entity kind, and templates are stored by name but deleted by id. |
| [108](108-serde-defaults-for-legacy.md) | open | Stored state carries `#[serde(default)]` only so that documents from before a field existed still load, although no such documents need to. |

## Sessions and channels

| No. | Status | Issue |
|---|---|---|
| [07](07-session-deletion-audit.md) | in progress | Deletion works since `540e482`; what remains open is whether and how a deletion is recorded. |
| [08](08-retry-and-edit-past-answers.md) | open | A conversation is append-only — no retry, no correcting a past message, so a derailed turn keeps influencing every later one. |
| [09](09-creating-channels.md) | done | Channels exist only as instantiated from a session template; there is no create, no close, and no way to add a participant afterwards. |
| [46](46-channel-participants.md) | done | The participants of a channel are fixed when it is built; nobody can be added to or removed from a running channel. |
| [70](70-channels-as-entities.md) | done | Channels are stored inside the session document, so every change to a channel has to go through the session and renaming a channel is not stored at all. |
| [73](73-watch-leaving-duplicates-events.md) | done | `ChannelLoop::watch_leaving` is a second channel for the fact that someone left, which the `EventBus` already announces as `UserChannelLeft`/`AgentChannelLeft`. |
| [67](67-change-channel-level.md) | open | A channel's confidentiality level can only be set when the channel is created, not changed afterwards. |
| [34](34-snapshot-on-stream-connect.md) | done | Event streams deliver only what happens after they connect, so a client cannot learn the current state and loses whatever happens while it connects. |
| [38](38-global-stream-loses-events-silently.md) | done | A client of the global event stream that falls behind loses events without being told, and its session list stays wrong until a reload. |
| [39](39-global-events-not-filtered.md) | done | The global event stream sends every user the events of every session, including the ids and new names of sessions they may not see. |
| [30](30-user-actor-for-authenticated-user.md) | done | Messages and approvals from the REST API go through the channel's first `UserActor`, so they are recorded in the name of someone other than the authenticated sender. |
| [42](42-export-import-chats.md) | open | Chats cannot be exported or imported, so a conversation cannot leave the server it happened on or be brought into another. |
| [75](75-agent-resumes-half-turn.md) | done | An agent stopped on purpose mid-turn resumes that turn like after a crash, and a resume in `ExecutingTools` re-runs its tools. |
| [76](76-channel-cancel-flag.md) | done | The channel's own cancel flag suppresses the final response of a cancelled turn, so the other agents' transcripts diverge from the history. |
| [79](79-user-actor-exits-on-send-error.md) | done | A `UserActor` ends itself when one forward fails, and the three actor run loops differ only in how they handle errors. |
| [90](90-message-path-wake-and-queue.md) | open | Frontends must wake the channel before sending, the `UserActor` queues every message once more than needed, and perceiving a message begins a turn. |

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
| [64](64-confidentiality-compartments.md) | open | Confidentiality has ordered levels only; information that may be shared between the channels of one topic or group but not others cannot be expressed. |
| [87](87-security-context-in-entities.md) | done | Stored agents and users carry a `SecurityContext` that the actors overwrite at start. |
| [94](94-users-unchecked-in-store.md) | open | The store checks access for every entity kind except users, where a placeholder comment stands in for the check. |
| [95](95-require-for-security-manager.md) | open | The runtime asks the `SecurityManager` for permission in four different ways with four different log lines. |

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
| [78](78-mcp-requires-approval-ignored.md) | done | `McpToolConfig.requires_approval` is never read; every discovered MCP tool runs without approval. |
| [81](81-approver-double-id.md) | done | The approver gives every approval a second id it never uses, and the agent passes through `ExecutingTools` only to start waiting. |
| [82](82-tool-parameter-parsing.md) | done | `create_plan` answers malformed arguments with "the tool failed" instead of "correct the arguments". |
| [100](100-tool-executor-table-and-result.md) | open | The tool executor scans all providers on every call, and a tool result exists in three shapes between the executor and the model. |
| [111](111-tool-parameter-reading.md) | done | Every built-in tool reads its parameters in its own way, so schema and reading code are written twice and can disagree. |
| [118](118-malformed-call-put-up-for-approval.md) | done | A tool call with invalid arguments is put up for approval as "No description" and fails only after a user approved it. |

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
| [56](56-response-language.md) | open | Agents often answer in English, and a user has no way to choose the language the agents of a channel answer in. |
| [80](80-stored-channel-response.md) | open | A channel that is not running is reported as not changeable and its agents' context as not readable, because the REST server picks and asks the access questions itself, in response builders spread over three modules. |
| [102](102-rest-channel-lookup.md) | open | The REST server repeats the session/channel lookup twelve times and decides itself what a viewer may see. |
| [103](103-rest-error-mappers.md) | open | Six REST functions map runtime errors to problem details with the same pattern. |
| [104](104-sse-stream-and-subscribe.md) | open | Both SSE handlers build their stream the same way, and the channel stream's membership loop is runtime knowledge in a handler. |
| [106](106-rest-small-findings.md) | open | Small inconsistencies in the REST server: hand-built responses, raw model types on the wire, stale docs, unused state. |

## Context and knowledge

| No. | Status | Issue |
|---|---|---|
| [16](16-context-compression.md) | open | History and context grow without bound; nothing decides what to drop before the model's context window is exceeded. |
| [17](17-information-sharing-within-session.md) | done | Agents in one session share nothing but routed messages — there is no scratchpad one can write to and another read. |
| [18](18-knowledge-sharing.md) | open | Sessions are sealed: nothing an agent learns outlives the session, and cross-agent sharing is a harder question again. |
| [19](19-memory-tiers.md) | open | The four memory tiers from `TODO.md` are named but undefined — scope, lifetime, readers and invalidation are all open. |
| [47](47-prompt-parts-with-provenance.md) | done | What a model is given besides the conversation is placed and recorded on two separate paths, and can only be a system prompt, whose every change invalidates the prompt cache. |
| [48](48-shared-digest-chain.md) | done | History and context duplicate one digest-chained structure, and notes would become a third copy that branching would have to change separately. |
| [50](50-turn-context-naming.md) | done | `TurnContext` calls both the stored context and the input of one model call "context". |
| [51](51-agent-private-notes.md) | done | An agent knows in each channel only what was said there; it has no private knowledge that carries over to its other channels. |
| [54](54-private-notes-visible-to-users.md) | done | Private notes meant for the agent alone are visible to users, in the context panel and as the tool call in the chat. |
| [55](55-notes-by-reflection.md) | open | Notes are written only when the model thinks of calling a note tool while answering, so what an agent learnt in a turn is kept unreliably and never tidied up. |
| [57](57-prompt-catalogue.md) | open | The prompts the runtime gives models are English constants spread through the code, in one variant for every language and model. |
| [58](58-notes-in-every-prompt.md) | open | Every note is put into every prompt in full, which grows without bound, and the writing agent sees its notes a second time as tool calls - revoked ones included. |
| [59](59-retrieval-of-notes.md) | open | Knowledge reaches the model only in full or when the model itself searches for it; nothing selects what is relevant to the turn. |
| [60](60-note-ids-in-prompts.md) | open | Notes of its own that an agent is shown from other channels carry no usable id, so it cannot revoke them, and ids from other chains are ambiguous. |
| [61](61-reasoning-leaves-the-agent.md) | done | An agent's reasoning travels with its messages to every user, and can disclose what a hidden tool call or private knowledge withholds. |
| [63](63-reasoning-in-model-input.md) | open | Every model call gets the reasoning of all earlier messages back, other agents' included, although several reasoning models document that it must not be. |
| [65](65-generic-component-state.md) | open | The state of notes is hard-wired into the agent state, the turn, the stores and REST, so every further kind of component state needs changes to the core and branching must be taught each one. |
| [66](66-channel-level-in-prompt.md) | done | Private notes and results of tools the channel does not see are not marked as private for the model, which may quote them in its always-published answer. |
| [72](72-history-digest-ignores-limit.md) | done | What `read_history` records as read names the whole history of the channel, although a reader who joined later with a limit saw only part of it. |
| [68](68-notes-in-agent-state.md) | done | An agent's notes are bound to one of its channels, although much of what it notes is knowledge of the agent itself, across channels and sessions. |
| [22](22-skill-management.md) | open | Skills do not exist: there is no unit of procedural knowledge below a personality, and no rule for who may load one into a turn. |
| [92](92-ledger-for-notes-and-memory.md) | open | `NoteChain` and `MemoryChain` are the same type written twice. |
| [98](98-context-provider-facet.md) | open | `ContextProvider` is a facet with one implementation, and a second one would silently overwrite the first. |
| [101](101-chain-component-helpers.md) | open | The three chain components copy their helpers, and `AgentNotes`/`AgentMemory` are constructed only to be called once. |
| [117](117-message-prev-id.md) | open | `Message::prev_id` is excluded from the digest because the chain owns it, yet two places still set it. |
| [119](119-seen-item-and-context-item.md) | done | What a model saw when it produced a message and what its turn gave it are two types of the same thing. |

## Maintenance

| No. | Status | Issue |
|---|---|---|
| [36](36-replace-serde-yaml.md) | done | The configuration is parsed with `serde_yaml`, which is deprecated and will receive no further fixes. |
| [71](71-access-helpers-on-entities.md) | done | Pure access questions such as `may_be_read_by`, `may_be_changed_by` and `history_visible_to` are public methods of the entities and bloat their interfaces. |
| [53](53-facet-trait-placement.md) | done | The traits components implement as facets live in different places, some among the implementations, and `MessageProcessor` names two unrelated traits. |
| [74](74-refactoring-epic.md) | open | Epic for the structure review of 2026-09-28: the bracket around issues 75–117 and their recommended order. |
| [96](96-config-unification.md) | done | `AppConfig` mixes tagged and untagged enums and `Option` with `default`. |
| [97](97-small-model-unifications.md) | open | Six helper functions in five modules define the default confidentiality level. |
| [112](112-id-types.md) | done | `ToolId` and `SessionTemplateId` are written by hand, and `ActorId::Approver` holds a bare `Uuid`. |
| [113](113-error-shapes.md) | done | "Forbidden" and "not found" have three shapes across the domain errors, and `ChannelError` lives in a crate that never constructs it. |
| [114](114-moderator-details.md) | done | `ModeratorActors` uses a concurrent map behind `&mut self`, downcasts for a kind the id already tells, and clones every message twice. |
| [115](115-filtering-event-sender.md) | done | `FilteringEventSender::send` drops a filtered envelope silently while `try_send` reports it as an error. |
| [105](105-seed-data-twice.md) | open | The demo data is seeded by both the REST server and the TUI, from two different definitions. |
| [109](109-code-hygiene.md) | open | `unwrap`, `unsafe`, infallible `Result`s, idle locks and per-chunk clones outside tests. |
| [110](110-docs-contradicting-code.md) | done | Doc comments and ARCHITECTURE.md name types and behaviour that the code does not have. |

## Code structure

| No. | Status | Issue |
|---|---|---|
| [83](83-turn-logic-on-agent-actor.md) | done | The logic of a turn lives in `AgentActor`, which drives `Turn` through eight to ten setters per step. |
| [84](84-agent-state-machine.md) | done | The agent's state machine has a state that only forwards and two handlers that differ in one branch. |
| [85](85-turn-state-in-agent-channel-state.md) | done | The working state of a turn is eleven loose fields of `AgentChannelState`, reset by three overlapping lists. |
| [86](86-controllers-in-the-model.md) | done | The five controllers write to the store and publish events from inside the model, each built differently. |
| [88](88-template-instantiation-in-the-model.md) | open | Instantiating a template does store I/O in the model, and both frontends drive the sequence by hand. |
| [89](89-channel-runtime-delegators.md) | open | `ChannelRuntime` is six methods that forward to `RuntimeManager`. |
| [91](91-double-actor-table.md) | done | `ChannelLoop` keeps its actors in two tables kept in step by a queue. |
| [99](99-component-factory-and-tags.md) | open | The component factory is synchronous while building is async, and component tags have no reader. |
| [107](107-public-interface.md) | open | Many `pub` items of the three crates have no caller, or one, outside their file. |
| [116](116-session-loop-channel-tables.md) | done | `SessionLoop` describes the one set of its channels with two tables and a queue. |

## Open architecture questions

| No. | Status | Issue |
|---|---|---|
| [20](20-transactional-effects.md) | open | A cognitive turn persists state, runs tools and emits events with no boundary around them; a crash mid-tool re-executes it on restart. |
| [21](21-scaling.md) | open | The runtime is single-process by design; which of the three scaling axes actually matters has never been decided. |
| [69](69-scoped-actor-runtime.md) | done | Every actor's `ActorRuntime` hands out the process-wide services unrestricted, so it cannot be given to components without letting them read around the confidentiality rules. |
