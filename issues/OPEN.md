# Open Issues

The issues that are `open` or `in progress`: the bugs first, then by area, those of priority
`low` last; within a section by priority, then by number. Generated from the field lines of the
issue files by `issues/generate.sh`, like [INDEX.md](INDEX.md) - never edit it by hand.

## Bugs

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [223](223-full-inbox-drops-events.md) | open | high | bug | An actor whose inbox is full - an agent in a long turn in a busy channel - loses what was said, requests and approvals without anyone learning of it. |
| [385](385-inbox-lost-on-restart.md) | open | medium | bug | An actor's inbox is a queue in memory, so a restart loses what was routed to it and not yet taken in - a change to a message, a request, a command - for good. |
| [143](143-joining-a-channel-with-foreign-knowledge.md) | open | low | bug | A participant added to a channel later sees what its agents read from other sessions, whose participants never agreed to be read by them. |

## Persistence

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [03](03-postgres-entity-store.md) | open | medium | feature | `EntityStore` has only in-memory and JSON-file implementations; a Postgres one is the prerequisite for everything multi-process. |
| [04](04-postgres-state-store.md) | open | medium | feature | `StateStore` likewise — and it holds the state whose loss is most expensive, including agents suspended on an approval. |

## Sessions and channels

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [381](381-inactive-channels-held-in-memory.md) | open | medium | feature | A channel nobody has spoken in for days keeps its loop, its actors and their states in memory until its session is stopped, and nothing notices that it is idle. Part of epic [378](378-resource-management-epic.md). |
| [382](382-agents-held-without-a-session.md) | open | medium | feature | An agent stays loaded with its memory while nothing uses it, and what still holds it once its sessions are unloaded has not been checked. Part of epic [378](378-resource-management-epic.md). |
| [388](388-resume-interrupted-work-after-a-restart.md) | open | medium | feature | Work interrupted by a restart - a turn stopped mid-step at the shutdown - waits until someone acts in its session, since nothing starts the session again. |

## Identity and access management

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [142](142-roles.md) | in progress | high | feature | Users carry roles as strings, but nothing says which roles exist or what they grant - clearances among them. |
| [169](169-changing-classified-content.md) | open | high | feature | Nothing says who may change something classified once it is written, or that what the writer has seen must fit into its classification. |
| [310](310-secrets-have-no-owner.md) | open | high | feature | Any secret can be named by whoever writes a component: secrets have no owner and no access list, and the policy allows every `ReadSecret`. |
| [402](402-login-attempts-unlimited.md) | open | high | feature | `/auth/login` takes any number of attempts from anyone, so passwords can be guessed without limit, and every attempt costs the server an Argon2 verification that a flood of requests turns into exhausted memory and CPU. |
| [27](27-ldap-user-backend.md) | open | medium | feature | Where identities are managed in a directory, the system has no way to authenticate against it or read its groups. |

## Audit

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [140](140-audit-log.md) | open | medium | feature | Acts that change who may see what - releases, participants, approvals, deletions - leave no record of who did them and when. |

## Tools and approvals

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [126](126-mcp-tools-discovered-once.md) | open | high | feature | An MCP server's tools are discovered once, when the agent is built, so a replaced or temporarily unreachable server is not followed. |
| [226](226-concurrent-tool-calls.md) | open | medium | feature | The tool calls of one step run one after the other, because every tool holds the turn while it runs, so a step of several slow calls takes the sum of their durations. |
| [419](419-approval-recorded-with-the-call.md) | open | medium | feature | Once a tool call was approved, nothing stored says who approved it or by which rule, so neither the chat nor the record of the turn can show it. |

## User interaction

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [208](208-generated-artifacts.md) | open | high | feature | Files and data an agent generates - an external agent's artifacts, a tool's images - are dropped; only their text is kept, and nothing can be stored or downloaded. |
| [282](282-message-attachments.md) | open | high | feature | A user cannot attach an image or a file to a message, so a model that can see, such as Gemma 4, is never shown one. |
| [298](298-pick-users-and-roles-to-share-with.md) | open | medium | feature | The web UI shares a workspace or an artifact only with the members of a session or channel, not with a user or a role by name, since the REST API lists neither. |

## Context and knowledge

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [58](58-notes-in-every-prompt.md) | open | high | feature | Every note is put into every prompt in full, which grows without bound, and the writing agent sees its notes a second time as tool calls - revoked ones included. |
| [338](338-artifacts-go-with-their-session.md) | open | high | feature | An artifact is published into its session's scope and is deleted with the session unless its owner keeps it elsewhere, though it is meant to outlive the conversation. |
| [55](55-notes-by-reflection.md) | open | medium | feature | Notes are written only when the model thinks of calling a note tool while answering, so what an agent learnt in a turn is kept unreliably and never tidied up. |
| [57](57-prompt-catalogue.md) | open | medium | question | The prompts the runtime gives models are English constants spread through the code, in one variant for every language and model. |
| [166](166-memory-as-topic-documents.md) | open | medium | feature | Agent memory is a flat list of one-line entries, all of them in every prompt, with nothing to group, revise or fetch them by topic. |

## Maintenance

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [378](378-resource-management-epic.md) | open | medium | epic | Epic for resource management: the bracket around 376, 379–383 and 218, which reduce what the server holds in memory, and their recommended order. |

## Low priority

| No. | Status | Priority | Type | Issue |
|---|---|---|---|---|
| [07](07-session-deletion-audit.md) | in progress | low | feature | Deletion works since `540e482`; what remains open is whether and how a deletion is recorded. |
| [26](26-database-user-backend.md) | open | low | feature | A file cannot serve several processes or an existing corporate user table; no database-backed user provider exists. |
| [42](42-export-import-chats.md) | open | low | feature | Chats cannot be exported or imported, so a conversation cannot leave the server it happened on or be brought into another. |
| [59](59-retrieval-of-notes.md) | open | low | feature | Knowledge reaches the model only in full or when the model itself searches for it; nothing selects what is relevant to the turn. |
| [122](122-several-instances.md) | open | low | feature | The server cannot run in several instances: everything live is in one process, and nothing keeps agents and users in step between processes. |
| [124](124-observer-participants.md) | open | low | feature | Nobody can join a channel just to watch it: joining makes a participant who can write, and without joining it cannot be followed live. |
| [139](139-mcp-result-classification.md) | open | low | feature | An MCP server cannot classify its results one by one, only be configured with one classification for all of them. |
| [173](173-message-inspection.md) | open | low | feature | Nothing examines what a message says before the channel routes it, so content that should be classified stricter, redacted or refused reaches every participant. |
| [181](181-where-roles-decide.md) | open | low | question | Nobody has reviewed where a user's roles - and soon an agent's - enter a permission check; today only the clearance at a channel's door asks for them. |
| [182](182-entity-reads-not-cached.md) | open | low | feature | Personalities, agents and users are read from the store again for every use - for every member of a channel, on every turn - though they change rarely. |
| [237](237-erasing-a-message.md) | open | low | feature | A message written by mistake can be taken back but not erased: it stays in every participant's record and in the contexts built on it. |
| [238](238-retry-of-an-older-answer.md) | open | low | feature | Only an agent's last answer can be asked for again, and the alternatives of an answer are not kept. |
| [252](252-openapi-tool.md) | open | low | feature | A tool server that describes itself by OpenAPI - as most HTTP services and Open WebUI's tool servers do - cannot be reached; only MCP servers can. |
| [256](256-workspace-versions.md) | open | low | feature | A workspace has no versions: what a person, a file tool or a command overwrote cannot be taken back, and who changed what is not recorded. |
| [272](272-artifacts-in-s3.md) | open | low | feature | Artifacts can be kept only on the server's own disk, not in S3 or another object storage. |
| [283](283-show-agent-components.md) | open | low | feature | The web UI does not show what an agent was built from, so its owner cannot see which components it has or which came from an included list. |
| [287](287-memberships-cache.md) | open | low | feature | Every check of a membership or a role in an access list reads the entity store, for every event of a session or channel and every subscriber. |
| [305](305-skill-versions.md) | open | low | feature | A skill keeps only its current text: every change overwrites what it said, nobody sees what changed, and an earlier version cannot be made current again. |
| [307](307-load-and-performance-tests.md) | open | low | feature | Nothing measures how Caelumora behaves under load, so its bottlenecks are guessed at, and a real model would hide them behind its own latency. |
| [332](332-policy-as-a-predicate.md) | open | low | feature | The policy decides entity by entity, so a store filters a list after reading it and no database could put who may read into its query. |
| [339](339-default-access-of-new-versions.md) | open | low | feature | Whom a new version of an artifact reaches is fixed to the channel it is published in, and neither a default for new versions nor a version's own list can be changed. |
| [340](340-teams-as-owners.md) | open | low | question | Only a person owns anything: no team can own what is done in its name, as a company does, so what is a team's result is one member's and goes with them. |
| [342](342-invitations.md) | open | low | feature | Nobody can be invited to a session or channel: who is added is taken in without being asked, told nothing beforehand and unable to decline. |
| [343](343-users-bring-their-own-secrets.md) | open | low | feature | A user cannot bring a secret of their own: every secret is the operator's, put into the configured providers, so an agent of one's own cannot reach a remote system with its owner's credential. |
| [346](346-subscription-topics-for-every-kind.md) | open | low | feature | The event bus has subscription topics only for sessions, channels, agents and users, so a plugin cannot follow a single entity - an artifact, say - without reading every event. |
| [413](413-compaction-restores-unused-skills.md) | open | low | feature | After a compaction, the `skills` component gives the agent again every skill loaded in the part that was summarised, whether the work still needs it or not, so a context just made smaller is filled with instructions from an earlier task. |
| [421](421-history-filter-as-data.md) | open | low | refactoring | `HistoryStore::page` takes `Keep`, a Rust closure, for what a viewer may read and what a search looks for; a database cannot turn it into a `WHERE`, so it has to become data. |
