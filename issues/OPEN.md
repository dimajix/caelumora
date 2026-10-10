# Open Issues

The issues of [INDEX.md](INDEX.md) that are `open` or `in progress`, under the same sections.
Generated from the index by `issues/open.sh` - never edit it by hand: change the index, and
the pre-commit hook (`.githooks/pre-commit`) or a run of the script brings this file up to date.

## Observability and operations

| No. | Status | Issue |
|---|---|---|
| [122](122-several-instances.md) | open | The server cannot run in several instances: everything live is in one process, and nothing keeps agents and users in step between processes. |
| [297](297-orderly-shutdown.md) | open | Caelumora ends without an orderly shutdown: a signal ends turns mid-step, breaks off streams and leaves the `local` shell's commands running. |

## Persistence

| No. | Status | Issue |
|---|---|---|
| [03](03-postgres-entity-store.md) | open | `EntityStore` has only in-memory and JSON-file implementations; a Postgres one is the prerequisite for everything multi-process. |
| [04](04-postgres-state-store.md) | open | `StateStore` likewise — and it holds the state whose loss is most expensive, including agents suspended on an approval. |
| [05](05-postgres-event-recorder.md) | open | The `Recorder` hook is wired into every channel, but `BlackholeRecorder` is the only implementation, so nothing is recorded. |
| [182](182-entity-reads-not-cached.md) | open | Personalities, agents and users are read from the store again for every use - for every member of a channel, on every turn - though they change rarely. |
| [295](295-access-as-a-query-condition.md) | open | A list asks the policy entity by entity, so a database store cannot push access into its query and a full page may be followed by an empty one. |

## Sessions and channels

| No. | Status | Issue |
|---|---|---|
| [07](07-session-deletion-audit.md) | in progress | Deletion works since `540e482`; what remains open is whether and how a deletion is recorded. |
| [42](42-export-import-chats.md) | open | Chats cannot be exported or imported, so a conversation cannot leave the server it happened on or be brought into another. |
| [124](124-observer-participants.md) | open | Nobody can join a channel just to watch it: joining makes a participant who can write, and without joining it cannot be followed live. |
| [143](143-joining-a-channel-with-foreign-knowledge.md) | open | A participant added to a channel later sees what its agents read from other sessions, whose participants never agreed to be read by them. |
| [173](173-message-inspection.md) | open | Nothing examines what a message says before the channel routes it, so content that should be classified stricter, redacted or refused reaches every participant. |
| [221](221-adding-session-participants.md) | open | A session's participants are fixed when it is created: nobody - neither a user nor an agent, existing or created from a personality - can be added to it later. |
| [223](223-full-inbox-drops-events.md) | open | An actor whose inbox is full - an agent in a long turn in a busy channel - loses what was said, requests and approvals without anyone learning of it. |
| [238](238-retry-of-an-older-answer.md) | open | Only an agent's last answer can be asked for again, and the alternatives of an answer are not kept. |

## Identity and access management

| No. | Status | Issue |
|---|---|---|
| [10](10-user-authentication.md) | in progress | There is no authentication at all; the REST server fabricates its own `SecurityContext` and `DummySecurityManager` approves everything. |
| [24](24-opa-security-manager.md) | open | `DummySecurityManager` is the only implementation; deployment-specific policy has no home outside the compiled binary. |
| [25](25-file-user-backend.md) | in progress | There is no source of identities to authenticate against; a hand-edited user file is the smallest one, and defines the provider seam. |
| [26](26-database-user-backend.md) | open | A file cannot serve several processes or an existing corporate user table; no database-backed user provider exists. |
| [27](27-ldap-user-backend.md) | open | Where identities are managed in a directory, the system has no way to authenticate against it or read its groups. |
| [125](125-invisible-admin.md) | open | There is no administrator who may see and follow every channel without taking part in it. |
| [140](140-audit-log.md) | open | Acts that change who may see what - releases, participants, approvals, deletions - leave no record of who did them and when. |
| [142](142-roles.md) | in progress | Users carry roles as strings, but nothing says which roles exist or what they grant - clearances among them. |
| [181](181-where-roles-decide.md) | open | Nobody has reviewed where a user's roles - and soon an agent's - enter a permission check; today only the clearance at a channel's door asks for them. |
| [169](169-changing-classified-content.md) | open | Nothing says who may change something classified once it is written, or that what the writer has seen must fit into its classification. |
| [286](286-groups-as-subjects.md) | open | A workspace or an artifact cannot be shared with a team that users make themselves, only with people one by one, a role, or the members of a session or channel. |
| [287](287-memberships-cache.md) | open | Every check of a membership or a role in an access list reads the entity store, for every event of a session or channel and every subscriber. |
| [289](289-attach-compares-access.md) | open | Attaching a workspace to a session shares it with that one session by an entry of its own, even where its access list already reaches everyone the session may take in. |

## Tools and approvals

| No. | Status | Issue |
|---|---|---|
| [126](126-mcp-tools-discovered-once.md) | open | An MCP server's tools are discovered once, when the agent is built, so a replaced or temporarily unreachable server is not followed. |
| [139](139-mcp-result-classification.md) | open | An MCP server cannot classify its results one by one, only be configured with one classification for all of them. |
| [226](226-concurrent-tool-calls.md) | open | The tool calls of one step run one after the other, because every tool holds the turn while it runs, so a step of several slow calls takes the sum of their durations. |
| [247](247-tool-own-matcher.md) | open | A rule on tool calls can match an argument that is a language - a shell command, SQL - only by its text, which a chained command slips past. |
| [252](252-openapi-tool.md) | open | A tool server that describes itself by OpenAPI - as most HTTP services and Open WebUI's tool servers do - cannot be reached; only MCP servers can. |
| [290](290-tools-offered-by-the-turn.md) | open | A model is offered every tool of its components in every turn, also those it may not call where it is - a workspace's tools where there is none, or where its list grants reading alone. |

## User interaction

| No. | Status | Issue |
|---|---|---|
| [208](208-generated-artifacts.md) | open | Files and data an agent generates - an external agent's artifacts, a tool's images - are dropped; only their text is kept, and nothing can be stored or downloaded. |
| [218](218-paged-histories.md) | open | A channel's history and an agent's history are returned whole over REST, though both grow without a limit and only their end is shown first. |
| [227](227-agent-management.md) | open | Agents cannot be seen or managed on their own, only as members of channels: no list, no renaming, no trash in the web UI, nothing of what an agent has across channels. |
| [282](282-message-attachments.md) | open | A user cannot attach an image or a file to a message, so a model that can see, such as Gemma 4, is never shown one. |
| [283](283-show-agent-components.md) | open | The web UI does not show what an agent was built from, so its owner cannot see which components it has or which came from an included list. |
| [291](291-share-artifacts-in-the-web-ui.md) | open | The web UI has no page of an artifact: its owner can neither see nor change whom it and its versions are shared with. |

## Context and knowledge

| No. | Status | Issue |
|---|---|---|
| [18](18-knowledge-sharing.md) | open | Sessions are sealed: nothing an agent learns outlives the session, and cross-agent sharing is a harder question again. |
| [19](19-memory-tiers.md) | open | The four memory tiers from `TODO.md` are named but undefined — scope, lifetime, readers and invalidation are all open. |
| [55](55-notes-by-reflection.md) | open | Notes are written only when the model thinks of calling a note tool while answering, so what an agent learnt in a turn is kept unreliably and never tidied up. |
| [57](57-prompt-catalogue.md) | open | The prompts the runtime gives models are English constants spread through the code, in one variant for every language and model. |
| [58](58-notes-in-every-prompt.md) | open | Every note is put into every prompt in full, which grows without bound, and the writing agent sees its notes a second time as tool calls - revoked ones included. |
| [59](59-retrieval-of-notes.md) | open | Knowledge reaches the model only in full or when the model itself searches for it; nothing selects what is relevant to the turn. |
| [65](65-generic-component-state.md) | open | The state of notes is hard-wired into the agent state, the turn, the stores and REST, so every further kind of component state needs changes to the core and branching must be taught each one. |
| [166](166-memory-as-topic-documents.md) | open | Agent memory is a flat list of one-line entries, all of them in every prompt, with nothing to group, revise or fetch them by topic. |
| [256](256-workspace-versions.md) | open | A workspace has no versions: what a person, a file tool or a command overwrote cannot be taken back, and who changed what is not recorded. |
| [272](272-artifacts-in-s3.md) | open | Artifacts can be kept only on the server's own disk, not in S3 or another object storage. |
| [237](237-erasing-a-message.md) | open | A message written by mistake can be taken back but not erased: it stays in every participant's record and in the contexts built on it. |
| [284](284-automatic-context-compaction.md) | open | An agent's context is compacted only when somebody asks for it; nothing compacts it before a model call that would not fit, least of all within the turn that makes it. |

## Open architecture questions

| No. | Status | Issue |
|---|---|---|
| [20](20-transactional-effects.md) | open | A cognitive turn persists state, runs tools and emits events with no boundary around them; a crash mid-tool re-executes it on restart. |
| [21](21-scaling.md) | open | The runtime is single-process by design; which of the three scaling axes actually matters has never been decided. |
