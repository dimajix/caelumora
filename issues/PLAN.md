# Plan: towards a first rounded release

Written 2026-10-05 from the 52 issues of [issues/OPEN.md](issues/OPEN.md). This is a direction, not
a schedule: it sorts the open issues by effort and value, names what is missing altogether, and
proposes an order. It decides nothing an issue file has to decide; where it recommends closing,
merging or rewriting an issue, that is a proposal for the maintainer.

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
calculate, plan, take notes, remember and call MCP tools - and nothing else
([TOOLS-AND-COMMANDS.md](docs/TOOLS-AND-COMMANDS.md#built-in-tools-and-commands)). Three gaps
stand out:

1.  **There is nothing to work on.** No workspace, no file, no artifact; what tools and external
    agents produce besides text is dropped (208). No open issue covers the workspace as such - see
    [block B1](#b1-workspace-and-artifacts---the-missing-subject).
2.  **A session ends at the context window** (16), and working on files reaches it fast: tool
    results are the bulk of a context.
3.  **Nobody can log in for real.** `accept_any` is the only user provider (25), so the server
    cannot leave a trusted machine.

## A. Small issues, little effort

Worth doing soon, in any order, alongside the blocks of section B.

| No. | Size | What is to do | Why now |
|---|---|---|---|
| [218](issues/218-paged-histories.md) | S | Page both histories alike, cut in the views. | Fully specified; long channels are already slow to open. |
| [56](issues/56-response-language.md) | S-M | A language on the channel, defaulting to its creator's preference, told by a prompt part. | Daily annoyance with local models; the recommendation is in the issue. |
| [150](issues/150-impersonation-towards-the-agent.md) | S | The two remainders: mark faked sender lines in the web UI, review which tools run without approval. | Everything else is decided; closes the issue. |
| [223](issues/223-full-inbox-drops-events.md) | M | Catch up what was said from the history; never drop a request or an approval. | Silent loss - and exactly the agents with long turns are hit, which working on files will produce. |
| [178](issues/178-scoped-lookup-by-name.md) | S now, M whole | Now: turn the scope check round and match the personality. The name space with shadowing and lifetime later, with 221. | The check finds another user's agent today; that part is a defect, not a feature. |
| [14](issues/14-approval-privilege.md) | S-M | Decide who is eligible (recommended in the issue: the owner of the agent whose call it is), count only the eligible. | Unblocked since 30; to be decided together with 165 and 13 (block B2). |
| [07](issues/07-session-deletion-audit.md) | S | `retention.purge_deleted_after` and the purge task. | Policy decided; only waits for 164. |
| [164](issues/164-soft-deletion-and-former-members.md) | M | Finish the remaining steps (former participants, views and REST, agents). | In progress; half-finished it is a liability. |
| [32](issues/32-unavailable-tools-invisible-to-model.md) | S | Tell the model which tool source is unavailable. | Small by itself, but its mechanism is 194's - do it there (block B4), not before. |

Decisions that cost an hour and close an issue are listed under [D](#d-questionable-issues).

## B. Important issues, large value

### B1. Workspace and artifacts - the missing subject

**Nothing exists, and no issue asks for it.** What exists are fragments that each assume it:

| No. | What it contributes | State |
|---|---|---|
| [208](issues/208-generated-artifacts.md) | An `ArtifactStore` for binary content, references in messages, download, a panel. | open, well specified |
| [169](issues/169-changing-classified-content.md) | The rule for changing something classified from several places: fixed container, the writer's label must flow into it, every change records where it was made. | open, proposal complete |
| [20](issues/20-transactional-effects.md) | Tools declare whether they have an effect or only read; what a crash in the middle means. | open, a question |
| [13](issues/13-approval-modes.md) | Remembered approvals scoped by tool *and argument shape*/compact - for files: by path. | open |
| [165](issues/165-ask-the-user-tool.md) | A presentation per kind of tool call in the web UI - "could later present a file diff". | open |
| [19](issues/19-memory-tiers.md), [MEMORY.md](docs/MEMORY.md) | The term: *workspace - documents several channels edit together; outside the branches, every change an effect.* | a table row |
| [166](issues/166-memory-as-topic-documents.md) | Notes that "Caelumora has no file system for agents". | - |

Issues to open (numbers to be assigned when they are created):

*   **W1 - What a workspace is** (architecture decision, first). A tree of files with
    `SecurityAttributes` and one classification (169's fixed container), in a scope like the
    definitions - of a session, or of a user and attached to sessions. To decide: where it is kept
    (a `WorkspaceStore` trait; a plain directory as the first implementation has the advantage
    that MCP servers, git and later a sandbox can work on the same files), who may attach it to
    which channel (the flow rule: a channel may read a workspace classified at or below it and
    write one at or above its turn label), and its lifetime (soft deletion as in 164).
    **208 must be decided with it, not beside it**: an artifact a tool generates and a file an
    agent writes are both named binary content with a producer and a classification. Two stores
    and two panels for them would be the repeated logic AGENTS.md warns of. Recommended: one
    store; a generated artifact is a file that a message refers to.
*   **W2 - Tools for agents**: a component `workspace_tools` (a `ToolProvider`): list, read (with
    ranges), search, write, edit by replacement, move, delete. Reads are recorded as knowledge
    (`KnowledgeRef`), writes are staged or effects (20), writing needs approval unless remembered
    (13).
*   **W3 - The users' side**: upload and download, a workspace panel in the web UI, attaching a
    file to a message, a diff presentation for write calls (on 165's handlers).
*   **W4 - Versions**: a workspace does not branch with the conversation, so regenerating (08)
    cannot take back an agent's bad edit. Each file keeps its versions with who wrote them from
    which turn (169), and a version can be restored. Without this, write tools are not safe to
    hand out.
*   **W5 - Later, deliberately not in the first release**: running things on the files (a
    sandboxed shell, code execution), converting formats (PDF, office documents) to text,
    images for models that see.

Order: decide **169** and the small part of **20** (tools declare *reads* or *has an effect*;
at-least-once made visible, nothing more) → **W1 with 208** → the refactoring of **226** (see B4)
→ W2 → W3 with 165's handlers → W4 → the rest of 208 (A2A and MCP parts taken in).

### B2. Working with an agent: asking and approving

| No. | Size | Note |
|---|---|---|
| [165](issues/165-ask-the-user-tool.md) | L | The design is nearly complete. Its second half - presentation handlers in the web UI - is infrastructure the workspace needs (diffs, attachments). |
| [13](issues/13-approval-modes.md) | M | "Always approve" with a scope is a precondition for write tools: ten prompts for ten edits train approving blindly. Brave mode is the cheap part. |
| [14](issues/14-approval-privilege.md) | S-M | Decide with 165 ("who may answer" is the same question). |

One body of work: 165 decides whether an answer *is* an approval; 14 says who may give either; 13
remembers one. Do them in that order.

### B3. Sessions that last: context and memory

| No. | Size | Note |
|---|---|---|
| [16](issues/16-context-compression.md) | L | The hard limit on every session. Start with the rule for old tool results and `/compact`; summarising after that. The mechanism it needs - a stored context that is not the transcript - is recorded in its history. |
| [58](issues/58-notes-in-every-prompt.md) | M | A budget for the prompt part and eliding committed note calls. Needs `KnowledgeRef` to name what was shown - the same refinement 166 and 59 need; build it once. |
| [169](issues/169-changing-classified-content.md) | M | Shared with B1; 166 waits for it. |
| [166](issues/166-memory-as-topic-documents.md) | L | Index in the prompt, document by tool. The pattern is also the skills' (22) and nearly the workspace's (W2): build "index in the prompt, body by tool" once. |
| [65](issues/65-generic-component-state.md) | M | Restructure-then-extend: finish it *before* 166 adds another kind of state to `AgentChannelChange`. |
| [55](issues/55-notes-by-reflection.md) | M | After 166: reflection needs documents it can revise as its target. Private notes only by default. |
| [22](issues/22-skill-management.md) | L | After 166, on its pattern. Says of itself that the runtime half is blocked on 12; instructions-only skills avoid most of that. Not in the first release. |

Order: 16 → 58 → (65) → 166 → 55 → 22. Only 16 and 58 belong to the first release.

### B4. Tools as something an agent visibly has

| No. | Size | Note |
|---|---|---|
| [194](issues/194-tool-sets.md) | M-L | Tool sets with a description, a prompt part, an agent-level view in the web UI. |
| [32](issues/32-unavailable-tools-invisible-to-model.md) | S | Falls out of 194: an unavailable source is a tool set that says so. |
| [126](issues/126-mcp-tools-discovered-once.md) | M | Refresh in the provider. With 32 and 194 one piece of work - "current, not offered". |
| [226](issues/226-concurrent-tool-calls.md) | M + M | Two steps. The first - tools read the turn and *return* their effects instead of holding `&mut Turn` - changes `ToolProvider::execute` for every provider. Do it **before** W2 adds a family of tools, and it also gives 20 its "declares an effect". Running concurrently is the second step and can wait. |
| [180](issues/180-outbound-flow-unchecked.md) | S-M | First question in the issue: is the coarse rule of 179 (the personality's roles at the door) enough? Probably yes for now - decide and document; per-tool clearance only if not. Matters more once agents hold files. |

### B5. Sessions that change, and a team that can use them

| No. | Size | Note |
|---|---|---|
| [25](issues/25-file-user-backend.md) | M | The file backend itself is still missing. Without it there are no passwords. With it: roles from the file (142's remainder) and the TUI's real identity (10's remainder) - three "in progress" issues close together. |
| [221](issues/221-adding-session-participants.md) | L | High value: today a session that needs one more agent is created anew. Shares "one way of creating an agent" with templates and needs 178's lookup. |
| [164](issues/164-soft-deletion-and-former-members.md) + [07](issues/07-session-deletion-audit.md) | M + S | See A. The purge needs an administrator: take **only** that part of [125](issues/125-invisible-admin.md) (the role, `ReadDeleted`/`Restore`/`Purge`), not the invisible live following. |
| [08](issues/08-retry-and-edit-past-answers.md) | XL | The most-used affordance of any chat, and the costliest issue here: 37 KB of plan, rewritten three times, step 1 of 7 done. Recommended cut for the first release: **regenerate the last answer in a channel with one user** - the case the issue itself names as the one where branching makes sense. Editing past messages, switching branches in shared channels and external agents after that. |
| [140](issues/140-audit-log.md) | M | Seven issues point at it (07, 13, 14, 125, 143, 164, 169). Not needed to *use* the system, but it is what makes the confidentiality model accountable. First thing after the first release - or earlier, if the release is to be shown to someone who asks "who released this". |

## C. Unimportant for now

Sound issues whose time has not come. None of them changes what a user can do with one server.

| No. | Why it can wait |
|---|---|
| [03](issues/03-postgres-entity-store.md), [04](issues/04-postgres-state-store.md) | The JSON stores carry one process. 04's shape is already decided (193); nothing is lost by waiting. |
| [122](issues/122-several-instances.md) | The model is agreed; building it needs 03 and 04 and a load that does not exist. |
| [26](issues/26-database-user-backend.md), [27](issues/27-ldap-user-backend.md) | The provider seam exists; 25 comes first. LDAP when a deployment asks for it. |
| [182](issues/182-entity-reads-not-cached.md) | Step 1 removed the duplicate reads. Caching pays with Postgres or large channels. |
| [139](issues/139-mcp-result-classification.md) | Small, but no MCP server classifies its results yet. Do it when one does. |
| [124](issues/124-observer-participants.md) | Nice for watching agents work among themselves; a participant who stays silent does the job meanwhile. |
| [125](issues/125-invisible-admin.md) | The live-following half. The role itself: see B5. |
| [42](issues/42-export-import-chats.md) | Useful for bug reports; the context panel covers reproducing for now. |
| [143](issues/143-joining-a-channel-with-foreign-knowledge.md) | A real gap, but it needs finer `KnowledgeRef`s (as 58) and the audit log (140) first. |
| [181](issues/181-where-roles-decide.md) | A review, not a feature. Do it when agents get roles that matter beyond the door - and take 12 into it (see D). |
| [20](issues/20-transactional-effects.md) | Beyond the small decision B1 needs: idempotency keys and transactions wait for a shared database. |

## D. Questionable issues

Issues whose concept is weak, overtaken or unproven. Each with a proposal; most can be settled by
a decision recorded in the issue file.

| No. | What is wrong with it | Proposal |
|---|---|---|
| [19](issues/19-memory-tiers.md) | An umbrella from `TODO.md`. Its principles are decided, its terms are in MEMORY.md, its phases ran under 17, 08 and 18. Its own *Done when* can no longer be told from the file. | Close as done with a pointer to MEMORY.md; the workspace row goes to W1. |
| [18](issues/18-knowledge-sharing.md) | Overtaken: agent memory (62, 68) already outlives sessions and deliberately does not follow the issue's "commit, not write". "An agent retrieves from its own past sessions" is true today; cross-agent sharing was never specified. | Rewrite to the one open half - knowledge shared *between* agents - or close and let 166 and the workspace answer it: a shared workspace is shared knowledge with an owner. |
| [21](issues/21-scaling.md) | A question that 122 answered: sessions in one instance, agents in several. | Close as decided, pointing to 122. |
| [05](issues/05-postgres-event-recorder.md) | Written before transcripts with digests (06), given-up turns (145) and the audit log (140) existed; they took its three purposes. What remains is a second, highly sensitive copy of every envelope. | Decide whether the `Recorder` hook still has a purpose; if not, reject and remove the hook rather than keep a `BlackholeRecorder` wired into every channel. |
| [12](issues/12-user-and-agent-permissions.md) | The problem statement predates ownership, classifications and clearances; the strict rule it called provisional was replaced by the flow rule. What is still open is "whose roles count when an agent acts" - which is 181's second bullet. | Merge into 181; keep the confused-deputy case as its test. |
| [23](issues/23-acls-in-security-attributes.md) | 20 KB designed when `SecurityAttributes` was one unread field. Since then: owner and contributors, roles configured by name in `AppConfig` (142) - which contradicts 23's decided `RoleId` as a UUID with stored assignments. | Do not implement from this text. Rewrite once a concrete need exceeds owner/contributor, starting from what 142 built. |
| [24](issues/24-opa-security-manager.md) | Rests on 23's attributes; solves a problem no deployment has. | Park behind 23. |
| [57](issues/57-prompt-catalogue.md) | Doubts itself ("first to be discussed is whether this is sensible at all") - rightly: n languages × m models without evaluations. | Decide as the issue's own *Order* suggests: built-in texts become component parameters where someone needs it; the catalogue is rejected. 56 solves the language problem separately. |
| [59](issues/59-retrieval-of-notes.md) | Embeddings, an index, finer provenance - for the notes of one session. 166's index already tells the model *that* it knows something, which is the weakness the issue names. | Park until a source exists that is too large for an index (documents in a workspace are the likely one, then as search in W2). |
| [173](issues/173-message-inspection.md) | Carefully designed, but expensive in the core: no streaming in an inspected channel, an inspection queue in the channel loop, mechanical repair of transcripts, an admitted hole for derived notes - for a control whose judge is a model and can be talked round (150 measured how easily). No user has asked for it yet. | Park. If content rules become a requirement, start with a deterministic inspector (patterns), which needs none of the hard parts' latency. |
| [08](issues/08-retry-and-edit-past-answers.md) | Not weak, but its scope is a risk: see B5. | Cut to the single-user case first. |
| [13](issues/13-approval-modes.md) | Brave mode and "always approve" in one issue, of which only the second has substance. | Keep; do the scope model first (B2), brave mode as its trivial special case or not at all. |

## Proposed order

Each milestone leaves the system usable; decisions come before the code that depends on them.

1.  **Clear the table.** The decisions of section D (19, 18, 21, 05, 12, 57, 59, 173 - no code),
    and the small ones: 218, 56, 150, 178 (the defect), 223.
2.  **A team can log in, and a session can change.** 25 with the remainders of 10 and 142; 164
    to the end, 07, the administrator role of 125; 221.
3.  **Agents ask, users approve sensibly.** 165 with the presentation handlers; 14; 13.
4.  **A workspace.** Decide 169, the small part of 20 and W1 together with 208; refactor
    `ToolProvider::execute` (226, first step); then W2, W3, W4, and the artifacts of 208.
5.  **Sessions that last.** 16 with `/compact`, 58; 194 with 32 and 126; 180 decided. If reading
    files hits the context window earlier, the tool-result rule of 16 moves into milestone 4.

**This is the first release.** After it, roughly in this order: 140 · 08 (single user) · 65 → 166
→ 55 · 22 · 226 (concurrency) · 124, 42 · and section C when a deployment asks.

Not covered by any issue and worth one each, besides W1-W5: **a second `LLMClient`**.
`ollama_client` is the only one; a client for the OpenAI-compatible API would open llama.cpp,
vLLM and hosted models without touching anything else.

## Open questions

The plan stands on assumptions the maintainer should confirm or correct:

1.  **The target.** One server, a small trusted team, local models - or is a deployment in an
    organisation (LDAP, Postgres, several instances) part of the *first* release? That would
    pull 03, 04, 27 forward and push the workspace back.
2.  **What a workspace holds.** Documents the agents write and revise (text, Markdown, data) - or
    also code that is to be built and run? The second needs a sandbox (W5) and changes W1: then
    the workspace must be a real directory from the start.
3.  **Whose workspace.** One per session, created with it - or a thing of its own that a user
    owns and attaches to several sessions (a project)? The second is the more useful and the
    larger one.
4.  **Branching.** Is "regenerate the last answer, one user" an acceptable first cut of 08, or is
    editing past messages the point?
5.  **Models.** Is Ollama enough for the first release, or is a second client part of it?

## Every open issue, by section

| Section | Issues |
|---|---|
| A - small | 07, 14, 32, 56, 150, 164, 178, 218, 223 |
| B1 - workspace | 20 (part), 169, 208, new W1-W5 |
| B2 - asking and approving | 13, 14, 165 |
| B3 - context and memory | 16, 22, 55, 58, 65, 166, 169 |
| B4 - tools | 32, 126, 180, 194, 226 |
| B5 - sessions and team | 07, 08, 10, 25, 140, 142, 164, 221, 125 (part) |
| C - not now | 03, 04, 20, 26, 27, 42, 122, 124, 125, 139, 143, 181, 182 |
| D - questionable | 05, 08 (scope), 12, 13 (brave mode), 18, 19, 21, 23, 24, 57, 59, 173 |
