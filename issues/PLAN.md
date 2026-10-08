# Plan: towards a first rounded release

Written from the open issues of [OPEN.md](OPEN.md), as they stand on 2026-10-07. This is a
direction, not a schedule: it sorts the open issues by effort and value, names what is missing
altogether, and proposes an order. It decides nothing an issue file has to decide; where it
recommends closing, merging or rewriting an issue, that is a proposal for the maintainer.

Efforts are rough sizes read off the issue files, not off the code: **S** (a day or two), **M**
(up to a week), **L** (several weeks, several steps), **XL** (a project of its own).

## The target

The plan assumes the following first release - if it is wrong, the order below changes (see
[Open questions](#open-questions)):

> A small, trusted team runs **one** Caelumora server with local models. People and several agents
> work together in channels **on files in a workspace**: agents read, write and revise documents,
> ask when they need a decision, are approved without wearing the users down, and a session can run
> for days and take in another person or agent on the way.

What this target does **not** need: several processes, Postgres, a directory service, an external
policy engine. The JSON stores and the ownership policy carry it.

Measured against that target, the system today is a well-secured chat: an agent run here can
calculate, plan, take notes, remember, ask the users and call MCP tools - and nothing else
([TOOLS-AND-COMMANDS.md](../docs/TOOLS-AND-COMMANDS.md#built-in-tools-and-commands)). As a chat it
is far along: messages can be corrected, taken back and answered again, approvals can be remembered
by rules of a place and of a person, a tool can ask its questions and frontends show its calls as
the tool declares. Three gaps stand out:

1.  **There is nothing to work on.** No workspace, no file, no artifact; what tools and external
    agents produce besides text is dropped (208). The one issue that asks for a workspace brings
    it from outside (251) - see [block B1](#b1-workspace-and-artifacts---the-missing-subject).
    *Since 2026-10-08:* a workspace exists (253, 251) - files people and agents share, a shell in
    a container - but keeps no versions (256), and artifacts are still dropped (208).
2.  **A session ends at the context window** (16), and working on files reaches it fast: tool
    results are the bulk of a context.
3.  **Nobody can log in for real.** `accept_any` is the only user provider (25), so the server
    cannot leave a trusted machine.

## A. Small issues, little effort

Worth doing soon, in any order, alongside the blocks of section B.

| No. | Size | What is to do | Why now |
|---|---|---|---|
| [218](218-paged-histories.md) | S | Page both histories alike, cut in the views. | Fully specified; long channels are already slow to open. |
| [223](223-full-inbox-drops-events.md) | M | Catch up what was said from the history; never drop a request, an approval or a question. | Silent loss - and exactly the agents with long turns are hit, which working on files will produce. A dropped change to a message is lost for good: catching up finds missing messages, not missed changes. |
| [07](07-session-deletion-audit.md) | S | `retention.purge_deleted_after` and the purge task. | Policy decided, and nothing blocks it: soft deletion and `ActionType::Purge` exist. |

Decisions that cost an hour and close an issue are listed under [D](#d-questionable-issues).

## B. Important issues, large value

### B1. Workspace and artifacts - the missing subject

**No issue asks for a workspace of Caelumora's own.** What exists are fragments that each assume
one, and one issue that brings a workspace from outside:

| No. | What it contributes | State |
|---|---|---|
| [253](253-workspace.md) | The workspace of Caelumora's own: a user's tree of files in a configured storage, used in a session through its channels, in the web UI. | done |
| [251](251-open-terminal.md) | Open Terminal: a container with files, a shell and processes, reached over REST; a component of its own with approval by kind of tool, long commands followed, a kill on interruption. | done - as the executor over 253, with `terminal_tool` and its file tools |
| [208](208-generated-artifacts.md) | An `ArtifactStore` for binary content, references in messages, download, a panel. | open, well specified; now: an artifact as a file of the session's workspace |
| [169](169-changing-classified-content.md) | The rule for changing something classified from several places: fixed container, the writer's label must flow into it, every change records where it was made. | open, proposal complete |
| [180](180-outbound-flow-unchecked.md) | What an agent sends out is checked by no rule; the proposal: a tool set whose remote side is not cleared for a channel is not offered there. | done |
| [247](247-tool-own-matcher.md) | A matcher that understands a shell command, so that "and from now on" does not allow chained commands. | open, its first tool exists (`run_command`) - next |
| [256](256-workspace-versions.md) | Versions of a workspace: what was overwritten can be taken back, who changed what is recorded. | open - W4 |
| [254](254-commands-run-unbounded.md) | A command handed back as running runs until the model ends it. | open; in part for `open_terminal_docker` |
| [20](20-transactional-effects.md) | Tools declare whether they have an effect or only read; what a crash in the middle means. | open, a question |
| [19](19-memory-tiers.md), [MEMORY.md](../docs/MEMORY.md) | The term: *workspace - documents several channels edit together; outside the branches, every change an effect.* | a table row |
| [166](166-memory-as-topic-documents.md) | Notes that "Caelumora has no file system for agents". | - |

**Two ways to a workspace.** 251 makes this the first decision of the block:

*   **Outside: Open Terminal's container is the workspace** (251 alone). Fast - 251 is M - and it
    covers code that is to be built and run. But the files belong to nobody in Caelumora: no
    `SecurityAttributes`, no owner, no versions, no download in the web UI, one classification per
    container by configuration; and the artifacts of 208 would need a store of their own anyway.
*   **Inside: our workspace (W1), Open Terminal as the executor over it** (recommended). The
    workspace is a directory with attributes and one classification; an Open Terminal container
    mounts that directory and runs commands on it. Reading and writing files is W2's alone, and
    251's tool set shrinks to the commands (`run_command`, `command_output`, `kill_command`) -
    otherwise two components offer the same file tools with two approval rules and two records.
    The cost: what a command changes passes W2's record and versions by, so W4 needs a snapshot
    around every command (git in the directory, say), not only around a write; and a container per
    workspace is something to operate.

251 can still come first, as the quick way to try agents on files, as long as its file tools are
understood as interim. *Decided 2026-10-08 (251, decision 4): inside - and built so.*

**One rule for all of them.** A workspace of classification W may be offered in a channel only
where it flows both ways: its content into the channel, and what the agent saw there into the
workspace - W1's flow rule, 251's "L = W", 180's clearance at the door. It is decided once, in 180,
for workspaces, MCP servers and Open Terminal alike.

W1 and W3 are issue [253](253-workspace.md), planned together with 251 (in 251's *Plan*), and W2 is
a step of that plan - all three done; W4 is issue [256](256-workspace-versions.md):

*   **W1 - What a workspace is** (architecture decision, first; now 253, done). A tree of files with
    `SecurityAttributes` and one classification (169's fixed container), in a scope like the
    scoped definitions - of a session, or of a user and attached to sessions. To decide: where it
    is kept (a `WorkspaceStore` trait; a plain directory as the first implementation, so that Open
    Terminal, MCP servers and git can work on the same files), who may attach it to which channel
    (the rule above), its lifetime (soft deletion), and the role of Open Terminal (above).
    **208 must be decided with it, not beside it**: an artifact a tool generates and a file an
    agent writes are both named binary content with a producer and a classification. Two stores
    and two panels for them would be the repeated logic AGENTS.md warns of. Recommended: one
    store; a generated artifact is a file that a message refers to.
*   **W2 - Tools for agents** (a step of 251's plan, done, as file tools of `terminal_tool`): list,
    read (with ranges), search, write, edit by replacement, move, delete. Reads are recorded as
    knowledge (`KnowledgeRef`), writes are staged or effects (20), writing needs approval unless a
    rule allows it - the tools propose rules by path (`ToolProvider::matcher`).
*   **W3 - The users' side** (now 253, done in part): upload and download, a workspace panel in
    the web UI, attaching a file to a message, a diff presentation for write calls (a
    `ToolPresentation` of its own). Attaching a file to a message and the diff presentation are not
    built, and no issue holds them yet.
*   **W4 - Versions** (now [256](256-workspace-versions.md)): a workspace does not branch with the
    conversation, so asking for an answer
    again cannot take back an agent's bad edit. Each file keeps its versions with who wrote them
    from which turn (169), and a version can be restored - around commands too, if Open Terminal
    runs on the workspace. Without this, write tools are not safe to hand out.
*   **Later, deliberately not in the first release**: converting formats (PDF, office documents) to
    text, images for models that see.

Order: decide **169**, the small part of **20** (tools declare *reads* or *has an effect*;
at-least-once made visible, nothing more), and **W1 with 208, 180 and the role of 251** together →
the refactoring of **226** (see B3) → W2 → **251** as the executor, with **247** → W3 → W4 → the
rest of 208 (A2A and MCP parts taken in). *As built:* W1, W3 in part, 180, 251 and W2 are done,
226 was not needed first; what remains is **247** → **W4 (256)** → the rest of W3 → 208.

### B2. Sessions that last: context and memory

| No. | Size | Note |
|---|---|---|
| [16](16-context-compression.md) | L | The hard limit on every session. Start with the rule for old tool results and `/compact`; summarising after that. The mechanism it needs - a stored context that is not the transcript - is recorded in the issue. |
| [58](58-notes-in-every-prompt.md) | M | A budget for the prompt part and eliding committed note calls. Needs `KnowledgeRef` to name what was shown - the same refinement 166 and 59 need; build it once. |
| [169](169-changing-classified-content.md) | M | Shared with B1; 166 waits for it. |
| [166](166-memory-as-topic-documents.md) | L | Index in the prompt, document by tool. The pattern is also the skills' (22) and nearly the workspace's (W2): build "index in the prompt, body by tool" once. |
| [65](65-generic-component-state.md) | M | Restructure-then-extend: finish it *before* 166 adds another kind of state to `AgentChannelChange`. |
| [55](55-notes-by-reflection.md) | M | After 166: reflection needs documents it can revise as its target. Private notes only by default. |
| [22](22-skill-management.md) | L | After 166, on its pattern. Says of itself that the runtime half is blocked on 12; instructions-only skills avoid most of that. Not in the first release. |

Order: 16 → 58 → (65) → 166 → 55 → 22. Only 16 and 58 belong to the first release.

### B3. Tools

| No. | Size | Note |
|---|---|---|
| [226](226-concurrent-tool-calls.md) | M + M | Two steps. The first - tools read the turn and *return* their effects instead of holding `&mut Turn` - changes `ToolProvider::execute` for every provider. Do it **before** W2 and 251 add families of tools, and it also gives 20 its "declares an effect". Running concurrently is the second step and can wait. |
| [126](126-mcp-tools-discovered-once.md) | M | Refresh in the provider; an MCP server's set already says when it is unavailable. What remains is following a server that changes or comes back - the same question for 252 once it exists. |

251, 247 and 180 are tools too, but belong to the workspace: see B1.

### B4. Sessions that change, and a team that can use them

| No. | Size | Note |
|---|---|---|
| [25](25-file-user-backend.md) | M | The file backend itself is still missing. Without it there are no passwords. With it: roles from the file (142's remainder) and the TUI's real identity (10's remainder) - three "in progress" issues close together. |
| [221](221-adding-session-participants.md) | L | High value: today a session that needs one more agent is created anew. Shares "one way of creating an agent" with templates; the lookup by name within a scope that it needs exists. |
| [07](07-session-deletion-audit.md) | S | See A. Purging by hand needs an administrator: take **only** that part of [125](125-invisible-admin.md) (the role, granted `ReadDeleted`/`Restore`/`Purge` - the actions exist, today only the system holds `Purge`), not the invisible live following. |
| [227](227-agent-management.md) | M | Agents on their own: a list, renaming, the trash. The backend of the trash exists; the web UI is missing. Belongs with 221 - both are about agents outside one channel. |
| [140](140-audit-log.md) | M | Seven issues point at it (07, 13, 14, 125, 143, 164, 169). Not needed to *use* the system, but it is what makes the confidentiality model accountable. First thing after the first release - or earlier, if the release is to be shown to someone who asks "who released this". |

## C. Unimportant for now

Sound issues whose time has not come. None of them changes what a user can do with one server.

| No. | Why it can wait |
|---|---|
| [03](03-postgres-entity-store.md), [04](04-postgres-state-store.md) | The JSON stores carry one process. 04's shape is already decided; nothing is lost by waiting. |
| [122](122-several-instances.md) | The model is agreed; building it needs 03 and 04 and a load that does not exist. |
| [26](26-database-user-backend.md), [27](27-ldap-user-backend.md) | The provider seam exists; 25 comes first. LDAP when a deployment asks for it. |
| [182](182-entity-reads-not-cached.md) | The duplicate reads are gone. Caching pays with Postgres or large channels. |
| [139](139-mcp-result-classification.md) | Small, but no MCP server classifies its results yet. Do it when one does. |
| [252](252-openapi-tool.md) | A generic OpenAPI tool is worth having, but has no concrete service waiting for it: Open Terminal is better served by 251. Do it when a second OpenAPI service is to be reached - that service decides the open questions (the default of the filter, API keys in a header). Until 251 is done it could bridge to Open Terminal; not worth building for that alone. |
| [124](124-observer-participants.md) | Nice for watching agents work among themselves; a participant who stays silent does the job meanwhile. |
| [125](125-invisible-admin.md) | The live-following half. The role itself: see B4. |
| [42](42-export-import-chats.md) | Useful for bug reports; the context panel covers reproducing for now. |
| [143](143-joining-a-channel-with-foreign-knowledge.md) | A real gap, but it needs finer `KnowledgeRef`s (as 58) and the audit log (140) first. |
| [181](181-where-roles-decide.md) | A review, not a feature. Do it when agents get roles that matter beyond the door - and take 12 into it (see D). |
| [20](20-transactional-effects.md) | Beyond the small decision B1 needs: idempotency keys and transactions wait for a shared database. |
| [238](238-retry-of-an-older-answer.md) | Retry of an older answer and keeping alternatives - branching. Asking for the last answer again covers what is asked most; 08's plan is the starting point when someone asks for more. |
| [237](237-erasing-a-message.md) | Erasing for good, beyond taking back. Analysed, not decided. A trusted team can live with "taken back"; data protection cannot. Decide it together with 05: a recorder that keeps every envelope has to support erasing, or 237 needs crypto-shredding. |

## D. Questionable issues

Issues whose concept is weak, overtaken or unproven. Each with a proposal; most can be settled by
a decision recorded in the issue file.

| No. | What is wrong with it | Proposal |
|---|---|---|
| [19](19-memory-tiers.md) | An umbrella from `TODO.md`. Its principles are decided, its terms are in MEMORY.md, its phases ran under 17, 08 and 18. Its own *Done when* can no longer be told from the file. | Close as done with a pointer to MEMORY.md; the workspace row goes to W1. |
| [18](18-knowledge-sharing.md) | Overtaken: agent memory already outlives sessions and deliberately does not follow the issue's "commit, not write". "An agent retrieves from its own past sessions" is true today; cross-agent sharing was never specified. | Rewrite to the one open half - knowledge shared *between* agents - or close and let 166 and the workspace answer it: a shared workspace is shared knowledge with an owner. |
| [21](21-scaling.md) | A question that 122 answered: sessions in one instance, agents in several. | Close as decided, pointing to 122. |
| [05](05-postgres-event-recorder.md) | Written before transcripts with digests, recorded given-up turns and the audit log (140) existed; they took its three purposes. What remains is a second, highly sensitive copy of every envelope - one that would also have to support erasing a message (237). | Decide whether the `Recorder` hook still has a purpose; if not, reject and remove the hook rather than keep a `BlackholeRecorder` wired into every channel. |
| [12](12-user-and-agent-permissions.md) | The problem statement predates ownership, classifications and clearances; the strict rule it called provisional was replaced by the flow rule. What is still open is "whose roles count when an agent acts" - which is 181's second bullet. | Merge into 181; keep the confused-deputy case as its test. |
| [23](23-acls-in-security-attributes.md) | 20 KB designed when `SecurityAttributes` was one unread field. Since then: owner and contributors, roles configured by name in `AppConfig` (142) - which contradicts 23's decided `RoleId` as a UUID with stored assignments. | Do not implement from this text. Rewrite once a concrete need exceeds owner/contributor, starting from what 142 built. |
| [24](24-opa-security-manager.md) | Rests on 23's attributes; solves a problem no deployment has. | Park behind 23. |
| [57](57-prompt-catalogue.md) | Doubts itself ("first to be discussed is whether this is sensible at all") - rightly: n languages × m models without evaluations. | Decide as the issue's own *Order* suggests: built-in texts become component parameters where someone needs it; the catalogue is rejected. The language problem is solved separately. |
| [59](59-retrieval-of-notes.md) | Embeddings, an index, finer provenance - for the notes of one session. 166's index already tells the model *that* it knows something, which is the weakness the issue names. | Park until a source exists that is too large for an index (documents in a workspace are the likely one, then as search in W2). |
| [173](173-message-inspection.md) | Carefully designed, but expensive in the core: no streaming in an inspected channel, an inspection queue in the channel loop, mechanical repair of transcripts, an admitted hole for derived notes - for a control whose judge is a model and can be talked round (150 measured how easily). No user has asked for it yet. | Park. If content rules become a requirement, start with a deterministic inspector (patterns), which needs none of the hard parts' latency. |

## Proposed order

Each milestone leaves the system usable; decisions come before the code that depends on them.

1.  **Clear the table.** The decisions of section D (19, 18, 21, 05, 12, 57, 59, 173 - no code),
    and the small ones: 218, 223.
2.  **A team can log in, and a session can change.** 25 with the remainders of 10 and 142; 07,
    the administrator role of 125; 221; 227.
3.  **A workspace.** Decide 169, the small part of 20, and W1 together with 208, 180 and the role
    of Open Terminal (251); refactor `ToolProvider::execute` (226, first step); then W2, 251 as the
    executor with 247, W3, W4, and the artifacts of 208. *Done: W1, W2, W3 in part, 180, 251;
    open: 247, W4 (256), the rest of W3, 208, 169, 20.*
4.  **Sessions that last.** 16 with `/compact`, 58; 126. If reading files hits the context window
    earlier, the tool-result rule of 16 moves into milestone 3.

**This is the first release.** After it, roughly in this order: 140 · 237 (with 05 decided) · 65 →
166 → 55 · 22 · 226 (concurrency) · 238 · 252 when a second OpenAPI service is to be reached ·
124, 42 · and section C when a deployment asks.

Not covered by any issue and worth one each, besides W1-W4: **a second `LLMClient`**.
`ollama_client` is the only one; a client for the OpenAI-compatible API would open llama.cpp,
vLLM and hosted models without touching anything else.

## Open questions

The plan stands on assumptions the maintainer should confirm or correct:

1.  **The target.** One server, a small trusted team, local models - or is a deployment in an
    organisation (LDAP, Postgres, several instances) part of the *first* release? That would pull
    03, 04, 27 forward and push the workspace back.
2.  **What a workspace holds.** *Answered by building both: files, and a shell where the storage
    gives one.* Documents the agents write and revise (text, Markdown, data) - or also code that is
    to be built and run? With 251 the second is no longer a project of its own: Open Terminal brings
    the sandbox. Either way the workspace should be a real directory from the start.
3.  **Whose workspace.** *Answered (251, decisions 12 to 14): a user's, in a scope, attached to
    sessions - or one of a session's own scope.* One per session, created with it - or a thing of
    its own that a user owns and attaches to several sessions (a project)? The second is the more
    useful and the larger one.
4.  **Open Terminal: the workspace, or the executor over ours?** *Answered: the executor.* The first
    is quicker, the second gives files an owner, versions and a place in the web UI (see B1).
    Recommended: the second, with 251 possibly first as an interim.
5.  **Models.** Is Ollama enough for the first release, or is a second client part of it?

## Every open issue, by section

| Section | Issues |
|---|---|
| A - small | 07, 218, 223 |
| B1 - workspace | 20 (part), 169, 208, 247, 254, 256 |
| B2 - context and memory | 16, 22, 55, 58, 65, 166, 169 |
| B3 - tools | 126, 226 |
| B4 - sessions and team | 07, 10, 25, 140, 142, 221, 227, 125 (part) |
| C - not now | 03, 04, 20, 26, 27, 42, 122, 124, 125, 139, 143, 181, 182, 237, 238, 252 |
| D - questionable | 05, 12, 18, 19, 21, 23, 24, 57, 59, 173, 255 |
