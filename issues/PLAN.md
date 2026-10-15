# Plan: towards a first rounded release

Written from the open issues of [OPEN.md](OPEN.md), as they stand on 2026-10-15. This is a
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
policy engine, invitations, teams that own things. The JSON stores and the ownership policy carry
it.

Measured against that target, three gaps remain:

1.  **Working on files is not yet safe to hand out.** A workspace keeps no versions (256): what an
    agent overwrote cannot be taken back - nor what it rewrote in a skill (305). A rule that allows
    a shell command matches only its text, so a chained command slips past it (247). What tools and
    external agents produce besides text is dropped (208), a user cannot show an agent an image
    (282), and an artifact, the result meant to outlive its conversation, is deleted with the
    session it was published in (338) - see [block B1](#b1-workspace-and-artifacts). This is the
    gap that weighs most.
2.  **Nobody can log in for real.** `accept_any` is the only user provider (25), so the server
    cannot leave a trusted machine.
3.  **What an agent knows grows without bound in every prompt.** Compaction shortens the
    transcript, but every note is put into every prompt in full (58); a session that runs for days
    fills its context with notes instead.

## A. Small issues, little effort

Worth doing soon, in any order, alongside the blocks of section B.

| No. | Size | What is to do | Why now |
|---|---|---|---|
| [218](218-paged-histories.md) | S | Page both histories alike, cut in the views. | Fully specified; long channels are already slow to open. |
| [223](223-full-inbox-drops-events.md) | M | Catch up what was said from the history; never drop a request, an approval or a question. | Silent loss - and exactly the agents with long turns are hit, which working on files produces. A dropped change to a message is lost for good: catching up finds missing messages, not missed changes. |
| [07](07-session-deletion-audit.md) | S | `retention.purge_deleted_after` and the purge task. | Policy decided, and nothing blocks it: soft deletion and `ActionType::Purge` exist. |
| [283](283-show-agent-components.md) | S | The web UI shows what an agent was built from, every included list resolved. | The runtime records what an agent was built from; only the web UI is missing. |
| [297](297-orderly-shutdown.md) | M | Catch SIGINT and SIGTERM, close the streams, stop the running sessions as `stop_session` does - their actors storing their state, their commands ended - within a bound; the same when the TUI ends. | A restart is an ordinary event for a server whose sessions run for days; today it ends turns mid-step and leaves the `local` shell's commands running. |
| [374](374-channel-controller-beside-the-actor-runtime.md) | S | The channel's controller in the `ChannelRuntime`, handed out by the `ActorRuntime` as the session's is; the actors, `AgentKinds::build` and `ToolExecutor::new` lose the parameter. | Every kind of agent a plugin registers carries the parameter; the fewer there are, the cheaper. |
| [375](375-test-runtimes-built-by-hand.md) | S | Survey what the twenty hand-built test runtimes set apart, then one fixture that takes a store and plugins; `outside_a_session` makes its empty `CommandExecutor` itself. | Before the next change to what a runtime is built from - 310 changes `ComponentRegistry::new` - which would otherwise touch twenty tests in four crates. |

Decisions that cost an hour and close an issue are listed under [D](#d-questionable-issues).

## B. Important issues, large value

### B1. Workspace and artifacts

| No. | What is to do | Note |
|---|---|---|
| [169](169-changing-classified-content.md) | The rule for changing something classified from several places: fixed container, the writer's label must flow into it, every change records where it was made. | Proposal complete. Skills raise their classification to what the changing turn has seen, while 169's *Done when* wants a classification to change "only by an act of its own" - decide 169 against that, before 256 and 305 record writes and 166 builds a third answer. |
| [247](247-tool-own-matcher.md) | A matcher that understands a shell command, so that "and from now on" does not allow chained commands. | Next: `run_command` exists, and without this a rule on it is unsafe. |
| [256](256-workspace-versions.md) | Versions of a workspace: each file keeps its versions with who wrote them from which turn (169), and a version can be restored. | A workspace does not branch with the conversation, so asking for an answer again cannot take back an agent's bad edit. Commands change files past the file tools, so a snapshot is needed around every command (git in the directory, say), not only around a write. Without this, write tools are not safe to hand out. |
| [305](305-skill-versions.md) | Versions of a skill: every change a new version with who made it, from where, under which label; diffs in the web UI; an earlier version made current by copying it. | The argument of 256 for what agents write with `update_skill`. Recommended, not decided: share the shape of a history with artifacts, not their store; versions as documents beside the skill. Brings a diff library to the web UI, which a diff presentation of write calls (below) can share. |
| [338](338-artifacts-go-with-their-session.md) | An artifact lies in its owner's scope, not its session's; another's artifact is named by its id; `keep` goes. | Deferred by the maintainer, the proposal complete. Decide it before 208 makes artifacts of what tools produce, every one of which would otherwise go with its session. The proposal holds for a team's scope as well, should 340 come. |
| [208](208-generated-artifacts.md) | Take in what A2A agents and MCP tools produce besides text - files, images - as artifacts, in the existing `ArtifactStorage`. | After 338. |
| [339](339-default-access-of-new-versions.md) | A default list on an artifact that a new version is given; the list of a version changed by its owner. | An owner who shared an artifact shares it again after every version today. Shown on the artifact's page in the web UI, with 298's picker. |
| [272](272-artifacts-in-s3.md) | An `s3` provider of `ArtifactStorage`. | Nothing blocks it; when a deployment asks. |
| [282](282-message-attachments.md) | A user attaches images to a message; the model that can see is given them, the one that cannot is told. | Decided in part: what is stored refers, the copies the model is given carry the content, fetched by the turn through a cache. Open: whether an attachment is part of its message or a file of the user's, as an artifact - which artifacts' lists, and those of their versions, would then decide who reads it. |
| [298](298-pick-users-and-roles-to-share-with.md) | Pick a user or a role in `AccessList`, from what the server lets an owner find. | Until then the web UI shares with the members of a session or channel and with teams, whose members are in turn chosen among those one shares a session with. Who may find whom is a decision of the security model. |
| - | A diff presentation for write calls (a `ToolPresentation` of its own). | No issue yet; worth one. |

**Deliberately not in the first release:** converting formats (PDF, office documents) to text.

Order: decide **169** and **338** → **247** → **256** → **305** → **282** → **208**; **298** and
**339** alongside; **272** when wanted.

### B2. Sessions that last: context and memory

| No. | Size | Note |
|---|---|---|
| [58](58-notes-in-every-prompt.md) | M | A budget for the prompt part and eliding committed note calls. Needs `KnowledgeRef` to name what was shown - the same refinement 166 and 59 need; build it once. |
| [169](169-changing-classified-content.md) | M | Shared with B1; 166 waits for it. |
| [166](166-memory-as-topic-documents.md) | L | Index in the prompt, document by tool - in the form skills have: a catalogue as a prompt part, a tool for the body, reads recorded as knowledge; the storage stays its own. The issue still says Caelumora has no file system for agents; correct that when it is taken up. |
| [55](55-notes-by-reflection.md) | M | After 166: reflection needs documents it can revise as its target. Private notes only by default. |

Order: 58 → 166 → 55. Only 58 belongs to the first release.

### B3. Tools

| No. | Size | Note |
|---|---|---|
| [226](226-concurrent-tool-calls.md) | M + M | Two steps. The first - tools read the turn and *return* their effects instead of holding `&mut Turn` - changes `ToolProvider::execute` for every provider, the file tools, `terminal_tool` and the skills' tools among them; every new tool makes it dearer. Running concurrently is the second step, in the agent's loop, and can wait. |
| [290](290-tools-offered-by-the-turn.md) | M | Offer a model only the tools it may call where it is - a workspace's tools where there is one, those that change where its list grants writing. A `ToolProvider` that knows the turn, asked where the turn's tool sets are built; the call keeps its own check. |
| [126](126-mcp-tools-discovered-once.md) | M | Refresh in the provider; an MCP server's set already says when it is unavailable. What remains is following a server that changes or comes back - the same question for 252 once it exists. |

247 is a tool too, but belongs to the workspace: see B1.

### B4. A team that can use it

| No. | Size | Note |
|---|---|---|
| [25](25-file-user-backend.md) | M | The file backend itself is missing. Without it there are no passwords. With it: roles from the file (142's remainder) and the TUI's real identity (10's remainder) - three "in progress" issues close together. |
| [310](310-secrets-have-no-owner.md) | M | Decided, with an implementation plan: secrets in vaults of the configuration, each with an access list and the origins its secrets may be sent to; a reference names its vault (`vault/KEY`). Latent while only the seed and the configuration write components - but it changes the form of the configuration and of stored personalities, which is cheaper before a first release has deployments. 343 builds on it. |
| [140](140-audit-log.md) | M | Seven issues point at it (07, 13, 14, 125, 143, 164, 169). Not needed to *use* the system, but it is what makes the confidentiality model accountable. First thing after the first release - or earlier, if the release is to be shown to someone who asks "who released this". |

## C. Unimportant for now

Sound issues whose time has not come. None of them changes what a user can do with one server.

| No. | Why it can wait |
|---|---|
| [03](03-postgres-entity-store.md), [04](04-postgres-state-store.md) | The JSON stores carry one process. 04's shape is decided; nothing is lost by waiting. |
| [122](122-several-instances.md) | The model is agreed, and events already cross instances in one form; building it needs 03 and 04 and a load that does not exist. |
| [307](307-load-and-performance-tests.md) | One server with a small team is no load. Worth building before 03 and 122, so that they are chosen by measurement; it would also make `caelumora-rest-client` a real client. Recommended there, not decided: Rust, a fake Ollama server in the suite. |
| [332](332-policy-as-a-predicate.md) | The policy as a condition a store can evaluate or translate. It pays once a store can execute a condition itself - with 03; the memory and JSON stores read every document anyway. Until then a full page may be followed by an empty one, which paging (218) has to tolerate. |
| [26](26-database-user-backend.md), [27](27-ldap-user-backend.md) | The provider seam exists; 25 comes first. LDAP when a deployment asks for it. |
| [287](287-memberships-cache.md) | A cache of memberships and roles for the policy - of groups, since a membership is read from the one group an entry names. Pays where many events and subscribers meet; correctness came first. |
| [182](182-entity-reads-not-cached.md) | Caching pays with Postgres or large channels. |
| [342](342-invitations.md) | Invitations to a session or channel, as personal `Join` entries. A trusted team lives with being taken in directly; an invitation matters where people who do not know each other share a server. Its list of open invitations wants 332's predicate. |
| [343](343-users-bring-their-own-secrets.md) | Secrets a user brings, in the vault `user`. After 310, and only once users write agents of their own that reach remote systems. |
| [346](346-subscription-topics-for-every-kind.md) | Topics on the event bus for every kind. No use case yet; opened to be referred to. |
| [139](139-mcp-result-classification.md) | Small, but no MCP server classifies its results yet. Do it when one does. |
| [252](252-openapi-tool.md) | A generic OpenAPI tool is worth having, but has no concrete service waiting for it: Open Terminal has a component of its own. Do it when a second OpenAPI service is to be reached - that service decides the open questions (the default of the filter, API keys in a header). |
| [124](124-observer-participants.md) | Nice for watching agents work among themselves; a participant who stays silent does the job meanwhile. |
| [42](42-export-import-chats.md) | Useful for bug reports; the context panel covers reproducing for now. |
| [143](143-joining-a-channel-with-foreign-knowledge.md) | A real gap, but it needs finer `KnowledgeRef`s (as 58) and the audit log (140) first. |
| [181](181-where-roles-decide.md) | A review, not a feature. Whose roles count when an agent acts is settled (its owner's, narrowed by its personality, carried by the `SecurityContext`), and where: wherever an entry of a list or a capability names a role (23). What remains is going through the actions once. |
| [238](238-retry-of-an-older-answer.md) | Retry of an older answer and keeping alternatives - branching. Asking for the last answer again covers what is asked most; 08's plan is the starting point when someone asks for more. |
| [237](237-erasing-a-message.md) | Erasing for good, beyond taking back. Analysed, not decided. A trusted team can live with "taken back"; data protection cannot. Decide it together with 05: a recorder that keeps every envelope has to support erasing, or 237 needs crypto-shredding. |

## D. Questionable issues

Issues whose concept is weak, overtaken or unproven. Each with a proposal; most can be settled by
a decision recorded in the issue file.

| No. | What is wrong with it | Proposal |
|---|---|---|
| [18](18-knowledge-sharing.md) | Overtaken: agent memory outlives sessions and deliberately does not follow the issue's "commit, not write". "An agent retrieves from its own past sessions" holds; cross-agent sharing was never specified. | Rewrite to the one open half - knowledge shared *between* agents - or close and let 166 and the workspace answer it: a shared workspace is shared knowledge with an owner. |
| [05](05-postgres-event-recorder.md) | Its three purposes are served by transcripts with digests, recorded given-up turns and the audit log (140). What remains is a second, highly sensitive copy of every envelope - one that would also have to support erasing a message (237). | Decide whether the `Recorder` hook still has a purpose; if not, reject and remove the hook rather than keep a `BlackholeRecorder` wired into every channel. |
| [57](57-prompt-catalogue.md) | Doubts itself ("first to be discussed is whether this is sensible at all") - rightly: n languages × m models without evaluations. | Decide as the issue's own *Order* suggests: built-in texts become component parameters where someone needs it; the catalogue is rejected. The language problem is solved separately. |
| [59](59-retrieval-of-notes.md) | Embeddings, an index, finer provenance - for the notes of one session. 166's index tells the model *that* it knows something, which is the weakness the issue names. | Park until a source exists that is too large for an index (documents in a workspace are the likely one, then beside the workspace's text search). |
| [173](173-message-inspection.md) | Carefully designed, but expensive in the core: no streaming in an inspected channel, an inspection queue in the channel loop, mechanical repair of transcripts, an admitted hole for derived notes - for a control whose judge is a model and can be talked round (150 measured how easily). No user has asked for it yet. | Park. If content rules become a requirement, start with a deterministic inspector (patterns), which needs none of the hard parts' latency. |
| [340](340-teams-as-owners.md) | A memo, not an issue: whether teams own things, as a company does, and who then acts in a team's name. Nothing decided, not even whether to do it. | Leave it a memo until shared administration is actually missed; then first the cheaper grant `Manage` in an access list, and ownership by teams only if a lifetime or a name space of a team's own is wanted. |

## Proposed order

Each milestone leaves the system usable; decisions come before the code that depends on them.

1.  **Clear the table.** The decisions of section D (18, 05, 57, 59, 173, 340 - no code), and
    the small ones: 218, 223, 297; 374 and 375 before the next change to what an actor or a
    runtime is built from.
2.  **A team can log in.** 25 with the remainders of 10 and 142; 07; 310.
3.  **Working on files, safely.** Decide 169 and 338; refactor `ToolProvider::execute` (226, first
    step); then 247, 256, 305, 282, 208, 290; 298 and 339 alongside.
4.  **Sessions that last.** 58; 126.

**This is the first release.** After it, roughly in this order: 140 · 237 (with 05 decided) · 166
→ 55 · 226 (concurrency) · 238 · 342, 343 · 307 before 03 and 122 · 272 and 252 when a deployment
asks · 124, 42 · and section C when a deployment asks - 332 with 03.

Not covered by any issue and worth one each:

*   **A second `LLMClient`.** `caelumora-ollama` is the only one; since the core names no model
    provider, a client for the OpenAI-compatible API is a plugin beside it and would open
    llama.cpp, vLLM and hosted models without touching anything else.
*   **A diff presentation for write calls** of the workspace, sharing the diff of the web UI with
    305.

## Open questions

The plan stands on assumptions the maintainer should confirm or correct:

1.  **The target.** One server, a small trusted team, local models - or is a deployment in an
    organisation (LDAP, Postgres, several instances) part of the *first* release? That would pull
    03, 04, 27, 332 and 307 forward and push the rest of the workspace back.
2.  **Models.** Is Ollama enough for the first release, or is a second client part of it?
3.  **Agents of one's own.** Do users write personalities and components themselves in the first
    release? Then 310 is a precondition rather than a convenience, and 343 moves up.

## Every open issue, by section

| Section | Issues |
|---|---|
| A - small | 07, 218, 223, 283, 297, 374, 375 |
| B1 - workspace | 169, 208, 247, 256, 272, 282, 298, 305, 338, 339 |
| B2 - context and memory | 55, 58, 166, 169 |
| B3 - tools | 126, 226, 290 |
| B4 - a team | 10, 25, 140, 142, 310 |
| C - not now | 03, 04, 26, 27, 42, 122, 124, 139, 143, 181, 182, 237, 238, 252, 287, 307, 332, 342, 343, 346 |
| D - questionable | 05, 18, 57, 59, 173, 340 |
