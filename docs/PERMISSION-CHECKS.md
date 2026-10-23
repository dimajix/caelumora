# Permission Checks

This document is the map of every place where the runtime asks a permission question: which
operation asks, in whose name, what, which concept decides it, by which of the five reasons it is
asked, and where the same question is asked again on the way. It records what the code does at the
end of issue [304](../issues/304-review-of-the-permission-concept.md) (2026-10-11), redone from the
code, and brought up to date by issues [312](../issues/312-order-and-reach-not-everywhere.md) and
[322](../issues/322-clearance-decided-outside-the-store.md). The rules it is measured against - the
three decisions, the order, the five reasons to ask - are in
[SECURITY.md](SECURITY.md#where-permission-checks-go) and [CONFIDENTIALITY.md](CONFIDENTIALITY.md);
what each action is expected to ask is asserted by the security suite
(`caelumora-runtime/tests/security_suite.rs`, issue 198; an agent's actions in
`caelumora-agents/tests/security_suite.rs`, issue 370). A new action, or a change to what one asks,
changes this map as it changes the suite.

## The concepts

Who may do what is decided by four concepts. Since issue 304 each has its own function:

| No. | Concept | Decided by | Asked as |
|---|---|---|---|
| 1 | **Ownership and access lists**: who may read, use, change or delete a stored thing | the `AccessPolicy`, from `SecurityAttributes` (`owner_id`, `acl` naming actors, roles and groups) and the context's principal; the `Admission` the policy gives for a verb (issue 332) | `Get*`, `Create*`, `Update*`, `Delete*`, `Write*`, `UseSkill`, `ReadDeleted`, `HearDeleted`, `Restore`, and the acts in a conversation (`CancelChannel`, `WakeChannel`, `RetryMessage`, `AnswerQuestion`) |
| 2 | **Flow and clearance**: whether information may go where the act takes it, and who is cleared for it | the `ConfidentialityPolicy` (`ConfidentialityRules`), from the roles in the context, the classifications and the places | `may_publish`, `may_copy_information`, `cleared_for`, and their `require_*` forms; `require_classifiable` is validation |
| 3 | **Rules of particular actions**: what is neither ownership nor flow | a rule written into the policy for the variant: authorship, who answers for an agent, the system alone; or nothing, the action being a hook | `ModifyMessage`, `RevokeMessage`, `Reclassify`, `ApproveToolCall`, `ShowAgentCommand`, `ExecuteAgentCommand`, `ShowCommand`, `ExecuteCommand`, `Purge`; `CallTool`, `ReadSecret` |
| 4 | **Reach**: whether a thing can be reached from where one stands | reading it (1) followed by the flow (2), one question (`confidentiality::may_reach`, issue 322): the store's for what is classified - `Get*` or `ReadDeleted`, then `cleared_for` (`Stored::classification`); for a channel read from a place `view::channel_reading` over a document read in S - `GetChannel`, then for a user the clearance, for an agent the copy into its place; for an artifact the store's, the views cutting the versions by their own lists (issue 316) | no action of its own; answered "not found" |

## Which concept decides each action

From `OwnershipPolicy::check` (`caelumora-model/src/security/ownership.rs`). The system principal
is allowed everything before any variant is looked at. "Principal" is the context's principal - a
user themselves, an agent its owner -, "caller" the one acting - the agent itself. What is done with
a stored entity is decided by the verb, never by the kind (issue
[323](../issues/323-kinds-enumerated-in-the-core.md)); a kind's own verb by what it requires.

| Actions | Rule | Concept |
|---|---|---|
| `Get` of every kind (`GetSession`, `GetSkill`, …); a kind's own verb requiring `Read` (`UseSkill`) | system-owned, or owned by the principal or the caller, or an entry grants `Read` to the principal or the caller (`Actor`, `Group`, `Role`) - the caller for an agent reading what it takes part in (issue 264) | 1 |
| `HearDeleted` of every kind | as reading, an entry for a deleted group counting as well | 1 |
| a kind's own verb requiring `Write` (`WriteWorkspace`, `WriteArtifact`) | as reading, with `Write` | 1 |
| `CancelChannel`, `WakeChannel`, `RetryMessage`, `AnswerQuestion` | the principal reads the channel | 1: reading grants acting in the conversation |
| `ReadAgentTools` | the principal reads the agent or the channel | 1, two objects |
| `ApproveToolCall` | `answers_for`: the principal reads the agent; for an agent of the system, owns the channel | 3, built from 1 |
| `ShowAgentCommand`, `ExecuteAgentCommand` | `owners_only`: `answers_for`; else the principal reads the channel | 3, built from 1 |
| `ShowCommand`, `ExecuteCommand` | `owners_only`: the principal owns the channel; else reads it | 3, built from 1 |
| `Create` of every kind | the new entity's owner is the principal | 1 |
| `Update`, `Delete`, `ReadDeleted`, `Restore` of every kind; a kind's own verb requiring `Owner`; `ReadAgentContext`, `ReadAgentComponents`, `StopSession` | the principal owns it | 1 |
| `Reclassify` | a channel its owner; a message the channel's owner or its author (the caller); a note or memory entry its author. Whether the new classification may be given there is the flow's | 1 or 3 |
| `ModifyMessage`, `RevokeMessage` | the author is the caller | 3: authorship, not ownership |
| `Purge` of every kind | nobody but the system (issue 125) | 3 |
| `List` of every kind | always; the store filters each item with `Get` | 1 per item |
| `CallTool`, `ReadSecret` | always: hooks for another `AccessPolicy`; a call is decided by the tools offered and the rules of its place, a secret has no owner yet (issue [310](../issues/310-secrets-have-no-owner.md)) | 3, decided elsewhere |

Most variants are decided by the principal; `ModifyMessage`, `RevokeMessage` and `Reclassify` of a
message, note or memory entry by the caller - an agent is judged as itself for what it said.
`Enforcement::require` logs a refusal with the principal either way, and hands the operation's
error what was refused (`Refused`, see [SECURITY.md](SECURITY.md)). The flow is not the
`AccessPolicy`'s at all (issue 304, reversing 138).

## The layers

| Layer | Asks | Form | Refusal |
|---|---|---|---|
| **Store** (`caelumora-model/src/store/access.rs`), rule 1 and reach | `Get*` on every read; `List*`, then `Get*` per item as a filter; `Create*` with the new attributes; `Get*` then `Update*`/`Delete*` on the stored ones; `ReadDeleted` then `Restore`/`Purge` - always Read before the verb; after the Read, for what is classified, `cleared_for` (issue 322) | `require`, `require_cleared_for`; `may_reach` as the list filter | `NotFound` for what may not be read or is not cleared for, `PermissionDenied` for what may not be changed, `Ok(false)` for what cannot be deleted |
| **Managers** (`SessionManager`, `SessionLoop`, `AgentManager`, `WorkspaceManager`, `ArtifactManager`, `SkillManager`, `GroupManager`, `SecretManager`), rules 2 and 4 | their verb before they act, where they act before they store; the flow of what they keep or make | `require`, `require_*` | the manager's error |
| **Channel loop** (`ChannelLoop`), rules 2 and 3 | what changes the channel, who is admitted, and what enters it (`admission`) | `require`, `require_*` | `ChannelError::Forbidden`, `NotCleared`; `MessageRejected` to the sender |
| **Owners of acts** (`ToolExecutor`, `CommandExecutor`, the agent carrying out a command, the approver, components on what they keep), rule 4 | the act's own question, once | `require` | the act's own answer: a forbidden result, `CommandFailed`, a left-out answer |
| **Views** (`caelumora-runtime/src/view`), rule 5 and reach | reaching (concept 4) and the flags of a snapshot | `may_*`; `require` for reading a participant's tools, components and context | `*NotFound`, a flag false, `Forbidden` |
| **Filters** (event bus, command lists, store lists), rule 5 | the read question per item | `check` | silently left out |

The REST server asks nothing itself. It builds one context per request (`auth.rs`, without a place:
no decision reads the place from the context, the flow is given its places as arguments), reaches
a session through `SessionManager::open` and a channel through `channel_for`, reads through the
views, and maps refusals to `403`, every `*NotFound` to `404`.

## The map

Notation for whose name: **U** the user's own context; **T** a turn's (the agent as caller, its
owner as owner and principal, whoever asked as initiator); **A** the agent's own (the agent itself
as initiator); **E** an envelope's, as its sender's actor set it - or, for an envelope an actor's
inbox read from the channel's history because its queue fell short (issue 223), the sender's context
as it is now, with the recorded initiator, and one without rights if the sender takes no part any
more; the one check it reaches is an agent's of a command; **V** a viewer's (U, or T with the place
the agent reads from); **C** the owner's of a channel or session, with their roles as they are when
it is built (its components, its commands - only while they are built); **S** the system's, which
asks nothing. After the name the concept. The rule column names the reason it is asked (R1-R5,
SECURITY.md); "store" is the store layer, in the context the caller passes. Every Read of the store
of what is classified - a channel, a workspace, a skill, an artifact - is followed by `cleared_for`
in the same context (issue 322), named in the rows where it is the reason of a refusal.

### Sessions

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Find (`SessionManager::find`, behind open, stop, delete) | running: `GetSession` (U,1) by hand; stored: store `GetSession` | R1 | by the views, if the client navigated first (R5) | `NotFound` |
| Make from a template (`create_from_template`) | store `GetSessionTemplate`; `CreateSession` (U,1), before anything is found or made (issue 312); a chosen workspace: reached - `GetWorkspace` (U,1), `cleared_for` (U,2) -, scope, `UpdateWorkspace` (U,1); its users resolved: store `GetUser`; store `GetUserProfile`; the template's workspace made (flow, then `CreateWorkspace`); then *Create* without its own `CreateSession`; then each agent found or made (*Make an agent*) and taken into the session (*Take someone in*); then the users and agents join the channels that name them as founders (`ChannelLoop::found`: `require_cleared` (U,2), an agent's `GetPersonality` (A,1) as its actor is built, nothing more - issue 420); the session taken back in S if any of it fails; then the workspace shared | R2 + R1 | `CreateSession` (pair); `UpdateWorkspace` twice (attach, store as it is shared) | `NotFound`, `PermissionDenied`, `PersonalityOutOfScope`, `WorkspaceOutOfScope`, `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `OutOfScope`; undone in S |
| Create (`establish`, behind a template; `create` in tests asks `CreateSession` first) | per channel `require_classifiable`; store `CreateSession`, per channel `CreateChannel` (U,1), the channels empty (issue 420); groups in S; in tests, `create` then has the participants of its channels join them as founders | R2 + R1; the members: others | `CreateSession` (pair); `GetPersonality` again as the channels are built | `ClassificationRefused`, `NotCleared`, `OutOfScope`, `PermissionDenied` |
| Open (`open`) | *Find*; channels loaded in S; per agent `GetPersonality` (A,1) - refused, the participant is left out; components built in C or A (`List` + `GetComponentBundle`, `ReadSecret`) | R1 | as creating | `NotFound` |
| Stop (`stop`) | *Find*; `StopSession` (U,1) | R4 | - | `NotFound`, `PermissionDenied` |
| Delete (`delete`) | *Find*; `DeleteSession` (U,1); channels in S; store `DeleteSession`; the scope listed in S, each item's store `Get…` + `Delete…` (U,1); the participants' groups deleted in S (issue 348) - something left, or the groups failing: the mark taken away in S; stopped | R2 + R1 | `DeleteSession` (pair) | `NotFound`, `PermissionDenied`, `ScopeNotEmpty` |
| Restore (`restore`) | store `ReadDeleted` (U,1), as whoever restores it; store `ReadDeleted` + `Restore`; the participants' groups restored in S (issue 348) - failing, the session marked again in S; the scope listed in S, each item's store `ReadDeleted` + `Restore` (U,1) - refused, it stays deleted | R1 | `ReadDeleted` (both store) | `NotFound`, `PermissionDenied` |
| Purge (`purge`) | store `ReadDeleted`; store `ReadDeleted` + `Purge`; channels and scope in S | R1 | - | `NotFound`, `PermissionDenied` |
| Rename, language, time zone, privacy, approval rules, description (`SessionLoop::update_*`) | store `GetSession` + `UpdateSession` (U,1) | R1 | - | `PermissionDenied` |
| Attach a workspace (`update_workspace`) | `UpdateSession` (U,1); reached: `GetWorkspace` (U,1), `cleared_for` (U,2), else `NotFound`; scope; `UpdateWorkspace` (U,1); store `UpdateSession`; sharing it as *Share* of a workspace | R2 + R1 | `UpdateSession` (pair); `UpdateWorkspace` twice | `PermissionDenied`, `NotFound`, `WorkspaceOutOfScope` |
| Take someone in (`SessionLoop::add_participant`) | `UpdateSession` (U,1) unless in the session and a channel is named; with a channel `channel_for` (U,1-2) and `UpdateChannel` (U,1); `GetUser` or `GetAgent` (U,1); `GetPersonality` (A,1); scope; `require_cleared` with the member's roles (U,2); store `GetSession` + `UpdateSession`; group in S; then the channel's `add_participant` | R2 + R1 | `UpdateSession`, `UpdateChannel` (pair) | `PermissionDenied`, `NotFound`, `AlreadyParticipant`, `CannotTakeIn`, `OutOfScope`, `NotCleared` |
| Join (`SessionManager::join`, `SessionLoop::join`) | `JoinSession` or `GetSession` (U,1), else `NotFound`; `GetUser` or `GetAgent` (U,1); `GetPersonality` (A,1); scope; `require_cleared` (U,2); `JoinSession` in the participant's context (M,1); `JoinChannel` (M,1); store in S; then the channel's `join` | R2 + R1 | `JoinSession`, `JoinChannel` (pair) | `NotFound`, `PermissionDenied`, `NotCleared` |
| Make an agent (`AgentManager::create`, behind a template and `POST /agents`) | `CreateAgent` (U,1); in a session's or a channel's scope store `GetSession` or `GetChannel` (U,1) - what lies there keeps the session from being deleted; store `GetPersonality`; scope; store `CreateAgent` | R2 + R1 | `CreateAgent` (pair) | `Invalid`, `NotFound`, `PermissionDenied`, `PersonalityOutOfScope`, `NameTaken` |
| Delete an agent (`AgentManager::delete`) | store `GetAgent`; `DeleteAgent` (U,1); store `DeleteAgent`; it rests in its channels, which follow it in S | R2 + R1 | `DeleteAgent` (pair) | `NotFound`, `PermissionDenied` |
| Restore, purge an agent (`AgentManager::restore`, `purge`) | store only | R1 | - | `NotFound`, `PermissionDenied` |
| Rename an agent (`AgentController::update_name`) | store `GetAgent` + `UpdateAgent` (U,1) | R1 | - | `NotFound`, `PermissionDenied`, `Invalid` |

### Channels

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Reach a channel to act in it (`SessionLoop::channel_for`, the REST server's way) | `channel_reading`: `GetChannel` (U,1), `cleared_for` (U,2) | reach | by the views if the client navigated | `ChannelNotFound` for absent, refused and not cleared alike |
| Create (`SessionLoop::create_channel`) | `require_classifiable`; store `GetUserProfile`; members of the session (structural); the component bundles named: `List` + `GetComponentBundle` (U,1, issue 321), and what the channel would not take refused (issue 337); `CreateChannel` (U,1), before the others (issue 312); `require_admitted` of the founders (A `GetPersonality`, scope, U cleared with the member's roles); store `CreateChannel` (U,1), the channel empty; group in S; store `GetSession` + `UpdateSession`; components built in C (`List` + `GetComponentBundle`, `ReadSecret`); then each founder joins (`ChannelLoop::found`, issue 420): cleared again (U,2), an agent's `GetPersonality` (A,1) as built, stored in S - no `UpdateChannel`; one that cannot: the channel taken back in S | R2 + R1 | `CreateChannel` (pair); `List` + `GetComponentBundle` (U, then C); `GetPersonality` as built | `InvalidComponents`, `PermissionDenied`, `ClassificationRefused`, `NotCleared`, `OutOfScope`, `NameTaken` |
| Delete (`SessionLoop::delete_channel`) | `channel_for`: `GetChannel` (U,1), `cleared_for` (U,2); `DeleteChannel` (U,1); store `UpdateSession` as the session drops it; store `GetChannel` + `DeleteChannel`; the scope listed in S, each item's store `Get…` + `Delete…` (U,1); its participants' group deleted in S (issue 348) - something left, or the group failing: listed and unmarked again in S; actors stopped | R2 + R1 | `GetChannel`, `DeleteChannel` (pair) | `ChannelNotFound`, `PermissionDenied`, `ScopeNotEmpty` |
| Restore (`SessionLoop::restore_channel`) | store `ReadDeleted` (U,1), as the session's trash lists it; `Restore` (U,1), before the others (issue 312); `require_admitted` as creating; store `ReadDeleted` + `Restore`; its participants' group restored in S (issue 348); store `GetSession` + `UpdateSession` - failing, unlisted and marked again in S, with its group; the scope as for a session | R2 + R1 | `ReadDeleted` (store), `Restore` (pair) | `ChannelNotFound`, `PermissionDenied`, `NotCleared`, `NameTaken` |
| Rename, language, description, workspace switch, approval rules (`ChannelLoop::update_*`) | store `GetChannel` + `UpdateChannel` (U,1) | R1 | - | `PermissionDenied` |
| Reclassify (`ChannelLoop::reclassify`) | `Reclassify` of the channel (U,1); `require_classifiable`; per participant `require_cleared` (U,2); store `UpdateChannel`; `may_publish` of the old into the new to learn whether it was lowered | R2 + R1; the last a question | - | `Forbidden`, `ClassificationRefused`, `NotCleared` |
| Add a participant (`add_participant`) | `UpdateChannel` (U,1); structural; `GetPersonality` (A,1); scope; `require_cleared` with the participant's roles (U,2); store `UpdateChannel`; the actor's components (A) | R2 + R1 | `UpdateChannel` (pair) | `Forbidden`, `PermissionDenied`, `OutOfScope`, `NotCleared` |
| Remove a participant (`remove_participant`) | structural (`NotAParticipant`); store `GetChannel` + `UpdateChannel` (U,1) | R1 | - | `PermissionDenied` |
| Cancel the channel, one actor, one tool call (`cancel`, `cancel_actor`, `interrupt_tool_call`) | `CancelChannel` (U,1) | R4 | - | `Forbidden` |
| Wake the channel, one actor (`wakeup`, `wakeup_actor`, from `/wakeup`) | `WakeChannel` (U,1) | R4 | `/wakeup` asked its own command questions first | `Forbidden` |

### What enters a channel

`ChannelLoop::admission`, in the envelope's context (**E**), in the loop, in order, against the
channel as it is when the event is taken in; a refusal is answered to the sender alone with
`MessageRejected`. The actors ask nothing for what they send. All rows R3; the sender's own
permission first, then the flow of what it carries.

| Event | Questions in order | `Rejection` |
|---|---|---|
| any event carrying messages | the sender is the caller (structural) | `Invalid` |
| a message, a turn's messages | per part - the message and each tool result - `require_publish` (E,2) | `ClassificationRefused` |
| `MessageModified` | the message is there; `ModifyMessage` (E,3); it can be changed so; the flow per part | `MessageNotFound`, `Forbidden`, `MessageUnchangeable`, `ClassificationRefused` |
| `MessageRevoked` | there; `RevokeMessage` (E,3); changeable | as above |
| `MessageReclassified` | there; `Reclassify` of the message (E,1 or 3); `require_publish` of the new classification (E,2); changeable | as above |
| `RetryRequested` | `RetryMessage` (E,1); a message, not taken back, or a turn, of an agent taking part | `MessageNotFound`, `Forbidden`, `MessageUnchangeable`, `Invalid` |
| `QuestionAnswered`, `QuestionDeclined` | `AnswerQuestion` (E,1); whether its sender was asked is the agent's | `Forbidden` |
| `CommandRequested`, `NudgeRequested` | form only: from a user, naming someone taking part; a command is the agent's to admit | `Invalid` |
| `ApprovalGranted`, `ApprovalDenied` from a user | structural: it says the sender's own part alone (`decisions`, issue 419); the agent it names read in S; `ApproveToolCall` (E,3); whether that agent asked is the approver's (issue 312) | `Invalid`, `Forbidden` |
| `ApprovalGranted`, `ApprovalDenied`, `ApprovalTimedOut` from anyone else | from an approver alone (structural) | `Invalid` |
| everything else | nothing | - |

Before sending, the `UserActor` refuses only what it can see at once: a message not there or taken
back, another's message to modify - a modification speaks as its author, which only the author's
actor can send (issue 228).

### Tool calls, approvals, commands, secrets

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Admit a tool call (`ToolExecutor::admit`) | offered; `CallTool` (T,3); the place's rules; approval | R4 | not again as it runs: offered, an agent, the place's rules | a forbidden result |
| A tool's result (`ToolExecutor::admitted`) | `may_publish` of the result into the turn (T,2) | R4: the agent's context is a destination of its own | the channel as the result enters it (R3) | the result withheld |
| Tool sets offered (`ToolExecutor::tool_sets`) | per set with a destination `may_publish` of the channel into it (T,2) | R5 | - | the set not offered |
| Who may approve (`ApproverActor::approvers_of`) | per user `may_approve` (their own context, 3) | R5 | - | left out; nobody, denied at once |
| An approval by click (`ApproverActor::handle_response`) | admitted by the channel (*What enters a channel*); the approver checks only that it names the agent that asked | R3 | `approvers_of` for the strategy (R5) | `MessageRejected` to its sender; an answer for another agent left out |
| An approval by a person's rule (`UserActor::answer_by_rule`) | `may_approve` (U,3), then as a click | R5 | three times per answer: whether the rule answers, the admission, whom to wait for | the rule does not answer |
| A command of the session (`CommandExecutor::execute`) | `ShowCommand`, `ExecuteCommand` with the channel (U,3) | R4 | `/wakeup` asks `WakeChannel` itself | `Unknown`, `Forbidden` |
| Commands shown (`visible_by`, `/help`, completion) | per command `ShowCommand` (U,3) | R5 | - | left out |
| A command of an agent asked for (`UserActor::request_command`) | whom it names: the agents' commands as the issuer sees them (`ShowAgentCommand`, U,3) | R5 | by the agent | `Unknown`, `InvalidArguments` |
| An agent carries a command out (`AgentBody::carry_out`) | `ShowAgentCommand`, `ExecuteAgentCommand` (E,3) | R4 | - | `CommandFailed` |
| A secret (`SecretManager::resolve`) | `ReadSecret` (3) - T at a call, A or C as a component is built, S for a storage | R4 | - | `SecretError::PermissionDenied` |

### What an agent reads beyond its channel

An agent's views are built per piece of work, with the agent's place; all in **T**.

| Operation | Questions in order | Rule | Refusal |
|---|---|---|---|
| Its own session (`ActorRuntime::session_view`) | `may_read_session` (T,1) - by the group of the session's participants | reach | `SessionNotFound` |
| A channel by name or id (`SessionView::channel`, `channel_named`; `read_history`, `read_notes`) | `channel_reading`: `GetChannel` (T,1), `copyable_information` of the channel into the place (T,2) - as a whole or entry by entry; entry by entry what it answers (`Reading`) judges each message or note by its classification, a page of the history as a filter in the store (issue 421) | reach | `ChannelNotFound`, `NoChannelNamed` alike |
| All channels (`channels`, `all_channels`) | as above per channel | reach | left out |
| Its own private notes of other channels (`own_private_notes`) | no `GetChannel` - its own knowledge -; per note `may_copy_from` (T,2) | reach | left out |
| Its memory (`AgentMemory::recalled`, `find`, `update`, `forget`) | per entry `may_copy_information` of its origin, by the origin's privacy then and now | R4: the agent's own knowledge, as its private notes | left out |
| Reclassify a note (`AgentNotes::reclassify`) | the note is there; `Reclassify` of the note with its channel's attributes (T,3); `require_publish` of the new classification (T,2) | R4 | `InvalidParameter`, `PermissionDenied` |
| Reclassify a memory entry (`AgentMemory::reclassify`) | the entry recalled; `Reclassify` with the channel's attributes (T,3); `require_publish` (T,2) | R4 | as above |

### Skills

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| A user lists or reads skills (`Views::skills`, `skill`) | store `GetSkill` (V,1), `cleared_for` (V,2) | R1 + reach (store) | - | left out; `SkillNotFound` |
| An agent's catalogue, `list_skills`, `load_skill`, `/skills`, `/skill`, compaction (`SkillViews::usable_skills`, `usable_skill_named`) | store `GetSkill` of what is seen from the place (T,1), `cleared_for` (T,2); `UseSkill` (T,1); `may_publish` into the place (T,2) | reach (store) | - | left out; `NoSkillNamed` |
| Create (`SkillManager::create`) | `require_classifiable`; `require_cleared_for` (U or T,2); store `CreateSkill` (1) | R4 (flow) + R1 | - | `ClassificationRefused`, `NotCleared`, `PermissionDenied`, `NameTaken` |
| Change, reclassify (`SkillManager::update`) | store `GetSkill`, `cleared_for` (2); for a classification `require_classifiable`, `require_cleared_for`; store `GetSkill`, `cleared_for` + `UpdateSkill` | reach (store) + R1 (+ flow) | `GetSkill`, `cleared_for` (store) | `NotFound`, `ClassificationRefused`, `NotCleared`, `PermissionDenied` |
| Delete, restore, purge (`SkillManager`) | store only: `GetSkill` or `ReadDeleted`, `cleared_for`, then the verb | R1 + reach (store) | - | `PermissionDenied`, `NotFound` |

### Workspaces

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| A user lists or reaches one (`Views::workspaces`, `workspace`) | store `GetWorkspace` (V,1), `cleared_for` (V,2) | R1 + reach (store) | by `reached` for a file route | left out; `WorkspaceNotFound` |
| A user works in it directly (`WorkspaceController::terminal`, the file routes) | store `GetWorkspace` (U,1), `cleared_for` (U,2), else `NotFound`; for a change `WriteWorkspace` (U,1) | reach (store), then R4 | - | `NotFound`, `ReadOnly` |
| In a channel (`WorkspaceController::usable_in_channel`, behind `ChannelLoop::workspace_terminal`, the terminal and artifact tools, the workspace prompt) | session and channel as the running session holds them, as stored for the `CommandReaper`; `may_read_session` (1); `channel_reading`; `usable_in`: the channel uses it, the caller takes part, scope, `may_read_workspace` (1), `may_publish` both ways (2); for a change `WriteWorkspace` (1) | reach, then R4 | - | `NotUsableInChannel`, `ReadOnly` |
| A running command goes on (`may_run`, as the channel's participants or the workspace change) | the chain of *In a channel* per current participant | an act that lasts, asked again as what permitted it changes | per participant and event | the command ended |
| The channel's workspace offered (`ChannelView::workspace`) | `usable_in` (V) | R5 | - | none |
| Create (`WorkspaceManager::create`, behind a template) | `require_classifiable`; `require_publish` into the storage's destination (U,2); `require_cleared_for` (U,2); in a session's scope `CreateWorkspace` (U,1) and store `GetSession` (U,1) - what lies there keeps the session from being deleted; store `CreateWorkspace` (U,1) | R4 (flow) + R1 | `CreateWorkspace` (pair, in a session's scope) | `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `PermissionDenied`, `NotFound`, `NameTaken` |
| Change, reclassify (`WorkspaceController::update_*`) | reached (store `GetWorkspace` U,1, `cleared_for` U,2); a classification as creating; store `GetWorkspace`, `cleared_for` + `UpdateWorkspace` | reach (store) + R1 (+ flow) | `GetWorkspace`, `cleared_for` (store) | `NotFound`, `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `PermissionDenied` |
| Share (`update_access`, `share`) | reached; `require_nameable`: store `GetGroup` per newly named group - the caller's own question (issue 312); store `GetWorkspace`, `cleared_for` + `UpdateWorkspace` | reach (store) + R1 | `GetWorkspace`, `cleared_for` (store) | `NotFound`, `PermissionDenied`, `Validation` |
| Delete, restore (`WorkspaceManager`) | delete: reached, else `false`; restore: the store alone - `ReadDeleted`, `cleared_for`, then the verb | R1 + reach (store) | `GetWorkspace`, `cleared_for` (store) for a deletion | `PermissionDenied`, `NotFound` |
| Purge (`purge`) | store `ReadDeleted`; `Purge` (U,3); files removed; store `ReadDeleted` + `Purge` | R2 + R1 | `Purge` (pair) | `PermissionDenied` |

### Artifacts

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| List, reach (`ArtifactViews::artifacts`, `artifact`, `ArtifactChannelViews::artifacts` of the plugin `caelumora-artifacts`) | listing: the teams the viewer belongs to, found as `Views::groups` finds them (V,1), named besides them and their roles (`Views::subjects`); store `GetArtifact` (V,1), `cleared_for` (V,2); the versions cut: per version `may_read_artifact` (`ArtifactAccess`) with the version's list (V,1) | R1 + reach (store) | - | left out; `ArtifactError::NotFound` |
| Where a version was published (`ArtifactView::publication`) | `Views::channel_of`: the session, then `channel_reading`; gone, as a live one would be reached | reach | - | not named |
| Publish (`ArtifactManager::publish`, the artifact tool) | the workspace's reach (*In a channel*); an existing artifact found in S: `WriteArtifact` (T,1) - one not the caller's to add to is refused as such, not taken for a name to make anew; or `CreateArtifact` (T,1); `require_publish` into the storage's destination (T,2); content kept; store `CreateArtifact` for a new one | R2 + R1 | `CreateArtifact` (pair) | `PermissionDenied`, `ClassifiedOtherwise`, `StorageNotCleared` |
| Share (`set_access`) | store `GetArtifact` (U,1), `cleared_for` (U,2); `require_nameable`; store `GetArtifact`, `cleared_for` + `UpdateArtifact` | reach (store) + R1 | `GetArtifact`, `cleared_for` (store) | `NotFound`, `PermissionDenied`, `Validation` |
| Keep in a scope (`keep`) | store `GetArtifact`, `cleared_for`; the scope compared with the owner (structural); store `GetArtifact`, `cleared_for` + `UpdateArtifact` | reach (store) + R1 | `GetArtifact`, `cleared_for` (store) | `NotFound`, `InvalidScope`, `PermissionDenied` |
| Describe (`describe`) | store `GetArtifact`, `cleared_for`; store `GetArtifact`, `cleared_for` + `UpdateArtifact` | reach (store) + R1 | `GetArtifact`, `cleared_for` (store) | `NotFound`, `PermissionDenied`, `Invalid` |
| Delete, restore (`ArtifactManager`) | the store: `GetArtifact` or `ReadDeleted`, `cleared_for`, then the verb; a deletion reads first to announce it | reach (store) + R1 | `GetArtifact`, `cleared_for` (store) for a deletion | `PermissionDenied`, `NotFound` |
| Purge (`purge`) | store `ReadDeleted`; `Purge` (U,3); content removed; store `ReadDeleted` + `Purge` | R2 + R1 | `Purge` (pair) | `PermissionDenied` |

### Groups

| Operation | Questions in order | Rule | Refusal |
|---|---|---|---|
| List, reach (`Views::groups`, `group`) | the teams the viewer owns or belongs to asked for alone (`Group::teams`, issue 317), store `GetGroup` (V,1) | R1 | left out; `GroupNotFound` |
| Make a team (`GroupManager::create`) | store `CreateGroup` (U,1) | R1 | `PermissionDenied`, `NameTaken` |
| Set members (`set_members`) | store `GetGroup`; a participants' group refused (structural); `UpdateGroup` (U,1), before whether the users exist is asked (issue 312); the users read in S; store `GetGroup` + `UpdateGroup` | R2 + R1 | `NotFound`, `Participants`, `PermissionDenied`, `UnknownMember` |
| Rename (`update`) | store `GetGroup`; a participants' group refused (structural); store `UpdateGroup` | R1 | `NotFound`, `Participants`, `PermissionDenied` |
| Delete, restore, purge | store; a participants' group refused | R1 | `NotFound`, `Participants`, `PermissionDenied` |
| Name a group in an access list (`require_nameable`) | store `GetGroup` (U,1) per newly named group | R1 | `Validation` |
| Drop entries for groups that are gone (`prune_gone_groups`, issue 347) | per group the lists name, read in S, also in the trash, as a workspace or an artifact is stored | - | - |
| The participants' groups (`keep_participants`, `delete_participants`, `restore_participants`, `purge_participants`) | in S, after the change of the participants, or the deletion, restoring or purge of their session or channel, was permitted | - | - |

### Views, snapshots and events

| Operation | Questions | Rule | Refusal |
|---|---|---|---|
| Sessions (`Views::sessions`, `session`) | store `GetSession`; running, `may_read_session` (V,1) | R1 + reach | left out; `SessionNotFound` |
| A deleted session or channel (`gone_session`, `gone_channel`) | read in S; `may_read_session`; a channel as a live one is reached (`SessionView::readable`, the clearance included) | reach | `Never` |
| Channels (`SessionView::readable`) | `channel_reading` (V) | reach | `ChannelNotFound` |
| Agents, personalities (`Views::agents`, `agent`, `personalities`) | store `GetAgent`, `GetPersonality` (V,1); an agent's personality named in S | R1 | left out; `AgentNotFound` |
| An agent's channels (`AgentView::channels`) | the sessions that have it, store `GetSession` (V,1); their channels as `SessionView::channels` reads them | R1 + reach | left out |
| Flags of a session, channel, member, agent, workspace, skill, group, artifact | `may_change_session`, `may_change_channel`, `may_read_agent_context`, `may_read_agent_tools`, `may_read_agent_components`, `may_approve`, `may_retry_message`, `may_cancel_channel`, `may_change_agent`, `may_delete_agent`, the member's commands, `may_change_workspace`, `may_write_workspace`, `may_change_skill` (`SkillAccess`), `may_change_group`, `may_change_artifact`, `may_write_artifact` (`ArtifactAccess`) | R5 | a flag false |
| A participant's tools, components, context, private notes (`tool_sets`, `components`, `context`) | `ReadAgentTools`, `ReadAgentComponents`, `ReadAgentContext` (V,1), the context also only of a channel read whole | R4: reading them is the views' act | `Forbidden` |
| What was deleted (`deleted_*`) | store `ReadDeleted` per item; flags `may_restore`, `may_purge` | R1 + R5 | - |
| An event reaching a subscriber (`EventBus`, `subscribe_as`) | per event and party `may_reach`, as the store asks it: the read question - `GetChannel` for a channel's events, its members' and an agent's state in it; `GetAgent`, `GetUser`, `GetUserProfile`, `GetWorkspace`, `GetSkill`, `GetGroup`, `GetArtifact` by kind; `GetSession` for a session's; `HearDeleted` instead for a deletion -, and for what is classified `cleared_for` the classification it has, or had as it is reclassified (issues 312, 322) | R5 + reach | not delivered |

### Users

| Operation | Questions | Rule | Refusal |
|---|---|---|---|
| A profile read or changed, approval rules (`UserController`) | store `GetUserProfile`, `UpdateUserProfile` (U,1) | R1 | `NotFound`, `PermissionDenied` |
| A channel marked read (`UserController::mark_read`) | store `GetChannel` (U,1), the clearance included; the message from where the user's view begins, structural; the state is the user's own, stored without a question | R1 | `NotFound` for the channel and the message alike |
| Users, agents, personalities, profiles read by the runtime (controllers, names in the views) | in S | - | - |
| Templates listed | store `GetSessionTemplate` per item (U,1) | R1 | left out |
| Roles, limiting roles | no question: carried by the context, put there by the `UserController` and the `AgentController` from the roles as they are now, the limit from the `PersonalityController` as it last took the personality in | - | - |

## Findings

### Decided to stay

- **Pairs (rule 2).** An operation that does something before it stores asks its verb, and the
  store asks again: deleting a session, a channel or an agent, sharing, purging, keeping an
  artifact. No token tells the store it was asked already.
- **The agents' own flow.** An agent asks the flow of what it gives up or brings in its own name -
  its turn still flows into the channel (`AgentActor::lowered`), an external agent's remote side,
  what a request brings, a lowered channel (`ExternalAgentActor`). That is the agent's act of
  withholding, not a pre-check of the channel's, which asks again as anything enters it (rule 3).
  Likewise a tool's result is asked into the agent's context before the channel asks it into
  itself: two destinations.
- **Memory.** An agent recalls its memory by the flow of each entry's origin itself - its own
  knowledge, reached through itself as its private notes of other channels are. Memory and notes
  are named in the core, each with its own enforcement; a generic one for every component's state
  was rejected (issue [65](../issues/65-generic-component-state.md)).
- **The reach is one function for offering and acting.** `channel_reading` and
  `WorkspaceController::usable_in` are built from `may_*` and answer with an option: the views turn
  `None` into "not there", an operation into "not found". It is neither enforcing nor offering, but
  the one way to reach a thing, so that both see the same.
- **A refusal that is no error.** The `ToolExecutor` asks `require` with a refusal of `()` and
  turns it into its own outcome, a forbidden result, so that the refusal is logged as every act's
  is.
- **Filters with `check`.** The event bus, the command lists and the store's list filter ask the raw
  question, since they take any action.
- **Lowered.** `ChannelLoop::reclassify` asks `may_publish` of the old classification into the new
  one to learn whether the channel was lowered - the question an agent asks at its next step, so
  that both agree.
- **Running commands.** A command in a workspace lasts; whether it may go on is asked again, in
  every participant's name, whenever the participants or the workspace change (`may_run`).
- **A new version of an artifact.** `publish` asks `WriteArtifact` of an artifact found in the
  runtime's name, so that one not the caller's to add to is refused as such rather than taken for a
  name to make anew.
- **`GetPersonality` twice.** An agent's personality is read as the participant is admitted and
  again as the channel is built: two acts.
- **The place in a REST context.** No decision reads it; the flow is given its places as arguments.
- **`CallTool` and `ReadSecret`** stay hooks for another `AccessPolicy`; secrets get an owner in
  issue [310](../issues/310-secrets-have-no-owner.md).

### Decided in issue 312

- **Validation and the actor's own flow before the verb.** What tells the actor only about their
  own request - its form, the structure of the reached object, the flow of what they ask for,
  whether they may read a group they name - may come before their permission, which the store
  then asks alone: the workspace, skill and artifact managers, `require_nameable`, `keep`. The
  group manager's `Participants` and `remove_participant`'s `NotAParticipant` are structure, seen
  by whoever reached the object.
- **Three `ApproveToolCall` for an answer by a rule.** Whether the person's rule answers at all
  (offering, so that no refusal is sent in their name), whether the channel admits the answer,
  and whom the approver waits for: three reasons, one each.
- **`UpdateWorkspace` twice for attaching and sharing.** Attaching asks before the session is
  changed, sharing is stored by the store's question alone.
- **A channel's components in its owner's name** are issue
  [321](../issues/321-channel-components-in-the-owners-name.md), decided below.
- **What is listed is the store's to decide.** A view does not filter a store's list again: the
  trashes show what `ReadDeleted` lets through, and restoring and purging reach it alike.

### Decided in issue 322

- **The store reaches what is classified, the clearance included.** Channels, workspaces, skills
  and artifacts answer their classification (`Stored::classification`), and the store asks
  `cleared_for` after every Read, the deleted alike. The views and the managers no longer ask it of
  what they read through the store; an operation asks it twice where it reads before it stores,
  as it asks `Get*` twice.
- **What the views read in the runtime's name stays theirs.** They read channels and artifacts in
  S and decide the reach themselves: a channel's depends on where the reader reads from - an agent
  reads one it is not cleared for as a whole entry by entry -, which a context does not carry; an
  artifact's on its versions' lists until issue
  [316](../issues/316-store-checks-artifact-without-versions.md).
- **One question of reach.** The store's list filter and the event bus ask
  `confidentiality::may_reach`, so that both reach alike.

### Decided in issue 321

- **Components are built in their owner's name.** An agent's in A, a channel's components and a
  session's commands in C, with the owner's roles as they are when they are built
  (`ControllerManager::owner_context`); an owner who is gone builds with none, as an agent whose
  owner is gone acts with none. Neither the channel as an actor of its own nor the system: the
  first would need rights given to channels, which nothing has, the second would ask nothing of
  what a configuration names.
- **The name is the build's alone.** What is asked in C is asked while building - a `ref`, a
  secret. No component keeps it; what it does later it does in the context of the act it serves:
  a moderator routes in the event's, a tool runs in the turn's. So a change of the owner's roles
  reaches the components when they are built again, as it reaches an agent's.
- **Whoever names a `ref` may read it.** The creator of a channel is not necessarily its owner; the
  component bundles they name are read in their name too, before the verb, as their own request
  (`ComponentRegistry::require_acceptable`).
- **Components nobody asks for are refused, not built** (issue
  [337](../issues/337-components-nobody-asks-for.md)): a container accepts only what its owner
  asks for and what that needs, and refuses the rest before anything is built in C - a tool in a
  channel reads no secret of its owner ([COMPONENTS.md](COMPONENTS.md#what-a-container-accepts)).
  A new channel is refused it before `CreateChannel` is asked, as its creator's `ref`s are read.

### Decided in issue 316

- **Reading a version needs the artifact's list and the version's**, as in HDFS and ADLS. The
  store checks an artifact by its own list, as every kind, and the views read artifacts in V, no
  longer in S; they cut the versions by their own lists (`Views::readable_artifact`). Publishing
  lets the channel's participants read the artifact as well as the version, in the runtime's name
  for whom `WriteArtifact` lets add a version. An event about a version reaches whoever reads both
  (`EventBus::publish_classified_part`). No method merges the lists for the store: a predicate
  over an entity's own attributes (issue [332](../issues/332-policy-as-a-predicate.md)) holds for
  artifacts as for every kind.

### Open

None from the map.
