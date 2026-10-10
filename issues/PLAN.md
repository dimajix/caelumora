# Plan: towards a first rounded release

Written from the open issues of [OPEN.md](OPEN.md), as they stand on 2026-10-10. This is a
direction, not a schedule: it sorts the open issues by effort and value, names what is missing
altogether, and proposes an order. It decides nothing an issue file has to decide; where it
recommends closing, merging or rewriting an issue, that is a proposal for the maintainer.

The plan looks only ahead. Its history is no part of it: what was done, when something was decided
and how the plan read before belong in the issue files and in git, not here. Every change to this
file keeps to that - a finished issue leaves the plan, a decision taken is stated as it stands now,
and no remark says what changed since an earlier version.

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

Measured against that target, three gaps remain:

1.  **Working on files is not yet safe to hand out.** A workspace keeps no versions (256): what an
    agent overwrote cannot be taken back. A rule that allows a shell command matches only its
    text, so a chained command slips past it (247). What tools and external agents produce
    besides text is dropped (208), and a user cannot show an agent an image (282) - see
    [block B1](#b1-workspace-and-artifacts).
2.  **A session ends at the context window** unless somebody types `/compact` in time (284), and
    working on files reaches it fast: tool results are the bulk of a context, and a long turn
    cannot be compacted from outside. This is the gap that weighs most.
3.  **Nobody can log in for real.** `accept_any` is the only user provider (25), so the server
    cannot leave a trusted machine.

## A. Small issues, little effort

Worth doing soon, in any order, alongside the blocks of section B.

| No. | Size | What is to do | Why now |
|---|---|---|---|
| [218](218-paged-histories.md) | S | Page both histories alike, cut in the views. | Fully specified; long channels are already slow to open. |
| [223](223-full-inbox-drops-events.md) | M | Catch up what was said from the history; never drop a request, an approval or a question. | Silent loss - and exactly the agents with long turns are hit, which working on files produces. A dropped change to a message is lost for good: catching up finds missing messages, not missed changes. |
| [07](07-session-deletion-audit.md) | S | `retention.purge_deleted_after` and the purge task. | Policy decided, and nothing blocks it: soft deletion and `ActionType::Purge` exist. |
| [283](283-show-agent-components.md) | S | The web UI shows what an agent was built from, every included list resolved. | The runtime records what an agent was built from; only the web UI is missing. |
| [297](297-orderly-shutdown.md) | M | Catch SIGINT and SIGTERM, close the streams, stop the running sessions as `stop_session` does - their actors storing their state, their commands ended - within a bound; the same when the TUI ends. | A restart is an ordinary event for a server whose sessions run for days; today it ends turns mid-step and leaves the `local` shell's commands running. |

Decisions that cost an hour and close an issue are listed under [D](#d-questionable-issues).

## B. Important issues, large value

### B1. Workspace and artifacts

| No. | What is to do | Note |
|---|---|---|
| [169](169-changing-classified-content.md) | The rule for changing something classified from several places: fixed container, the writer's label must flow into it, every change records where it was made. | Proposal complete. Skills raise their classification to what the changing turn has seen, while 169's *Done when* wants a classification to change "only by an act of its own" - decide 169 against that, before 256 records writes and 166 builds a third answer. |
| [20](20-transactional-effects.md) (part) | Tools declare whether they have an effect or only read; at-least-once made visible, nothing more. | The rest of 20 waits (see C). |
| [247](247-tool-own-matcher.md) | A matcher that understands a shell command, so that "and from now on" does not allow chained commands. | Next: `run_command` exists, and without this a rule on it is unsafe. |
| [256](256-workspace-versions.md) | Versions of a workspace: each file keeps its versions with who wrote them from which turn (169), and a version can be restored. | A workspace does not branch with the conversation, so asking for an answer again cannot take back an agent's bad edit. Commands change files past the file tools, so a snapshot is needed around every command (git in the directory, say), not only around a write. Without this, write tools are not safe to hand out. |
| [208](208-generated-artifacts.md) | Take in what A2A agents and MCP tools produce besides text - files, images - as artifacts, in the existing `ArtifactStorage`. | |
| [272](272-artifacts-in-s3.md) | An `s3` provider of `ArtifactStorage`. | Nothing blocks it; when a deployment asks. |
| [282](282-message-attachments.md) | A user attaches images to a message; the model that can see is given them, the one that cannot is told. | Decided in part: what is stored refers, the copies the model is given carry the content, fetched by the turn through a cache. Open: whether an attachment is part of its message or a file of the user's, as an artifact - which artifacts' lists, and those of their versions, would then decide who reads it. |
| [289](289-attach-compares-access.md) | Attaching a workspace compares its access list with the session's: no entry for the session where the list reaches its members already. | Spares lists of single sessions; needs the session's admission rule (221) for a comparison by subjects. |
| [291](291-share-artifacts-in-the-web-ui.md) | A page of an artifact in the web UI: its versions, and for its owner whom it and its versions are shared with. | Sharing is REST alone until then; the workspace page's `AccessList` is the pattern. |
| - | A diff presentation for write calls (a `ToolPresentation` of its own). | No issue yet; worth one. |

**Deliberately not in the first release:** converting formats (PDF, office documents) to text,
images for models that see.

Order: decide **169** and the small part of **20** → **247** → **256** → **282** → **208**; **291**
alongside; **289** after 221; **272** when wanted.

### B2. Sessions that last: context and memory

| No. | Size | Note |
|---|---|---|
| [284](284-automatic-context-compaction.md) | M | The hard limit on every session. The mechanism and `/compact` exist; what is missing is compacting on the agent's own, before a model call that would not fit - within a turn above all. Decide there whether dropping old tool results comes before summarising. |
| [58](58-notes-in-every-prompt.md) | M | A budget for the prompt part and eliding committed note calls. Needs `KnowledgeRef` to name what was shown - the same refinement 166 and 59 need; build it once. |
| [169](169-changing-classified-content.md) | M | Shared with B1; 166 waits for it. |
| [65](65-generic-component-state.md) | M | Restructure-then-extend: finish it *before* 166 adds another kind of state to `AgentChannelChange`. |
| [166](166-memory-as-topic-documents.md) | L | Index in the prompt, document by tool - in the form skills have: a catalogue as a prompt part, a tool for the body, reads recorded as knowledge; the storage stays its own. The issue still says Caelumora has no file system for agents; correct that when it is taken up. |
| [55](55-notes-by-reflection.md) | M | After 166: reflection needs documents it can revise as its target. Private notes only by default. |

Order: 284 → 58 → (65) → 166 → 55. Only 284 and 58 belong to the first release.

### B3. Tools

| No. | Size | Note |
|---|---|---|
| [226](226-concurrent-tool-calls.md) | M + M | Two steps. The first - tools read the turn and *return* their effects instead of holding `&mut Turn` - changes `ToolProvider::execute` for every provider, the file tools, `terminal_tool` and the skills' tools among them; every new tool makes it dearer. It also gives 20 its "declares an effect". Running concurrently is the second step, in the agent's loop, and can wait. |
| [290](290-tools-offered-by-the-turn.md) | M | Offer a model only the tools it may call where it is - a workspace's tools where there is one, those that change where its list grants writing. A `ToolProvider` that knows the turn, asked where the turn's tool sets are built; the call keeps its own check. |
| [126](126-mcp-tools-discovered-once.md) | M | Refresh in the provider; an MCP server's set already says when it is unavailable. What remains is following a server that changes or comes back - the same question for 252 once it exists. |

247 is a tool too, but belongs to the workspace: see B1.

### B4. Sessions that change, and a team that can use them

| No. | Size | Note |
|---|---|---|
| [25](25-file-user-backend.md) | M | The file backend itself is missing. Without it there are no passwords. With it: roles from the file (142's remainder) and the TUI's real identity (10's remainder) - three "in progress" issues close together. |
| [221](221-adding-session-participants.md) | L | High value: a session that needs one more agent has to be created anew. Shares "one way of creating an agent" with templates; the lookup by name within a scope that it needs exists. Comes with the session's admission rule decided in 23 (decision 5): from its template, empty for nobody, which its owner may only narrow. |
| [07](07-session-deletion-audit.md) | S | See A. Purging by hand needs an administrator: take **only** that part of [125](125-invisible-admin.md) - the role, granted `ReadDeleted`/`Restore`/`Purge` as capabilities (23, decision 4); the actions exist, only the system holds `Purge` -, not the invisible live following. |
| [227](227-agent-management.md) | M | Agents on their own: a list, renaming, the trash. The backend of the trash exists; the web UI is missing. Belongs with 221 - both are about agents outside one channel. |
| [140](140-audit-log.md) | M | Seven issues point at it (07, 13, 14, 125, 143, 164, 169). Not needed to *use* the system, but it is what makes the confidentiality model accountable. First thing after the first release - or earlier, if the release is to be shown to someone who asks "who released this". |

## C. Unimportant for now

Sound issues whose time has not come. None of them changes what a user can do with one server.

| No. | Why it can wait |
|---|---|
| [03](03-postgres-entity-store.md), [04](04-postgres-state-store.md) | The JSON stores carry one process. 04's shape is decided; nothing is lost by waiting. |
| [122](122-several-instances.md) | The model is agreed; building it needs 03 and 04 and a load that does not exist. |
| [26](26-database-user-backend.md), [27](27-ldap-user-backend.md) | The provider seam exists; 25 comes first. LDAP when a deployment asks for it. |
| [286](286-groups-as-subjects.md) | Groups of our own as subjects of access lists: sharing with a team rather than with people one by one. Wanted once several workspaces are shared with the same people. |
| [287](287-memberships-cache.md) | A cache of memberships and roles for the policy. Pays where many events and subscribers meet; correctness came first. |
| [182](182-entity-reads-not-cached.md) | Caching pays with Postgres or large channels. |
| [295](295-access-as-a-query-condition.md) | Access as a condition of a list's query, stated by the policy. It pays once a store can execute a condition itself - with 03; the memory and JSON stores read every document anyway. Until then a full page may be followed by an empty one, which paging (218) has to tolerate. |
| [139](139-mcp-result-classification.md) | Small, but no MCP server classifies its results yet. Do it when one does. |
| [252](252-openapi-tool.md) | A generic OpenAPI tool is worth having, but has no concrete service waiting for it: Open Terminal has a component of its own. Do it when a second OpenAPI service is to be reached - that service decides the open questions (the default of the filter, API keys in a header). |
| [124](124-observer-participants.md) | Nice for watching agents work among themselves; a participant who stays silent does the job meanwhile. |
| [125](125-invisible-admin.md) | The live-following half. The role itself: see B4. |
| [42](42-export-import-chats.md) | Useful for bug reports; the context panel covers reproducing for now. |
| [143](143-joining-a-channel-with-foreign-knowledge.md) | A real gap, but it needs finer `KnowledgeRef`s (as 58) and the audit log (140) first. |
| [181](181-where-roles-decide.md) | A review, not a feature; whose roles count when an agent acts is settled (its owner's, narrowed by its personality), and where: wherever an entry of a list or a capability names a role (23). What remains is going through the actions once 23 is built. |
| [20](20-transactional-effects.md) | Beyond the small part B1 needs: idempotency keys and transactions wait for a shared database. |
| [238](238-retry-of-an-older-answer.md) | Retry of an older answer and keeping alternatives - branching. Asking for the last answer again covers what is asked most; 08's plan is the starting point when someone asks for more. |
| [237](237-erasing-a-message.md) | Erasing for good, beyond taking back. Analysed, not decided. A trusted team can live with "taken back"; data protection cannot. Decide it together with 05: a recorder that keeps every envelope has to support erasing, or 237 needs crypto-shredding. |

## D. Questionable issues

Issues whose concept is weak, overtaken or unproven. Each with a proposal; most can be settled by
a decision recorded in the issue file.

| No. | What is wrong with it | Proposal |
|---|---|---|
| [19](19-memory-tiers.md) | An umbrella from `TODO.md`. Its principles are decided and its terms are in MEMORY.md; its own *Done when* can no longer be told from the file. | Close as done with a pointer to MEMORY.md. |
| [18](18-knowledge-sharing.md) | Overtaken: agent memory outlives sessions and deliberately does not follow the issue's "commit, not write". "An agent retrieves from its own past sessions" holds; cross-agent sharing was never specified. | Rewrite to the one open half - knowledge shared *between* agents - or close and let 166 and the workspace answer it: a shared workspace is shared knowledge with an owner. |
| [21](21-scaling.md) | A question that 122 answers: sessions in one instance, agents in several. | Close as decided, pointing to 122. |
| [05](05-postgres-event-recorder.md) | Its three purposes are served by transcripts with digests, recorded given-up turns and the audit log (140). What remains is a second, highly sensitive copy of every envelope - one that would also have to support erasing a message (237). | Decide whether the `Recorder` hook still has a purpose; if not, reject and remove the hook rather than keep a `BlackholeRecorder` wired into every channel. |
| [24](24-opa-security-manager.md) | A policy outside the binary, for a problem no deployment has; deny entries, which 23 leaves out, are what it would add. | Park until a deployment asks. |
| [57](57-prompt-catalogue.md) | Doubts itself ("first to be discussed is whether this is sensible at all") - rightly: n languages × m models without evaluations. | Decide as the issue's own *Order* suggests: built-in texts become component parameters where someone needs it; the catalogue is rejected. The language problem is solved separately. |
| [59](59-retrieval-of-notes.md) | Embeddings, an index, finer provenance - for the notes of one session. 166's index tells the model *that* it knows something, which is the weakness the issue names. | Park until a source exists that is too large for an index (documents in a workspace are the likely one, then beside the workspace's text search). |
| [173](173-message-inspection.md) | Carefully designed, but expensive in the core: no streaming in an inspected channel, an inspection queue in the channel loop, mechanical repair of transcripts, an admitted hole for derived notes - for a control whose judge is a model and can be talked round (150 measured how easily). No user has asked for it yet. | Park. If content rules become a requirement, start with a deterministic inspector (patterns), which needs none of the hard parts' latency. |

## Proposed order

Each milestone leaves the system usable; decisions come before the code that depends on them.

1.  **Clear the table.** The decisions of section D (19, 18, 21, 05, 57, 59, 173 - no code), and
    the small ones: 218, 223, 297.
2.  **A team can log in, and a session can change.** 25 with the remainders of 10 and 142; 07, the
    administrator's capabilities of 125; 221 with the admission rule; 227.
3.  **Working on files, safely.** Decide 169 and the small part of 20; refactor
    `ToolProvider::execute` (226, first step); then 247, 256, 282, 208, 290, 291; 289 after 221.
4.  **Sessions that last.** 284, 58; 126. If reading files hits the context window earlier, 284
    moves into milestone 3.

**This is the first release.** After it, roughly in this order: 140 · 237 (with 05 decided) · 65 →
166 → 55 · 226 (concurrency) · 238 · 272 and 252 when a deployment asks · 124, 42 · and section C
when a deployment asks - 295 with 03.

Not covered by any issue and worth one each:

*   **A second `LLMClient`.** `ollama_client` is the only one; a client for the OpenAI-compatible
    API would open llama.cpp, vLLM and hosted models without touching anything else.
*   **A diff presentation for write calls** of the workspace.

## Open questions

The plan stands on assumptions the maintainer should confirm or correct:

1.  **The target.** One server, a small trusted team, local models - or is a deployment in an
    organisation (LDAP, Postgres, several instances) part of the *first* release? That would pull
    03, 04, 27 forward and push the rest of the workspace back.
2.  **Models.** Is Ollama enough for the first release, or is a second client part of it?

## Every open issue, by section

| Section | Issues |
|---|---|
| A - small | 07, 218, 223, 283, 297 |
| B1 - workspace | 20 (part), 169, 208, 247, 256, 272, 282, 289, 291 |
| B2 - context and memory | 55, 58, 65, 166, 169, 284 |
| B3 - tools | 126, 226, 290 |
| B4 - sessions and team | 07, 10, 25, 140, 142, 221, 227, 125 (part) |
| C - not now | 03, 04, 20, 26, 27, 42, 122, 124, 125, 139, 143, 181, 182, 237, 238, 252, 286, 287, 295 |
| D - questionable | 05, 18, 19, 21, 24, 57, 59, 173 |
