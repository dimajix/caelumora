# Permission Checks

This document is the map of every place where the runtime asks a permission question: which
operation asks, in whose name, what, which concept decides it, by which of the five reasons it is
asked, and where the same question is asked again on the way. It records what the code does at the
end of issue [304](../issues/304-review-of-the-permission-concept.md) (2026-10-11), redone from the
code. The rules it is measured against - the three decisions, the order, the five reasons to ask -
are in [SECURITY.md](SECURITY.md#where-permission-checks-go) and
[CONFIDENTIALITY.md](CONFIDENTIALITY.md); what each action is expected to ask is asserted by the
security suite (`caelumora-runtime/tests/security_suite.rs`, issue 198). A new action, or a change
to what one asks, changes this map as it changes the suite.

## The concepts

Who may do what is decided by four concepts. Since issue 304 each has its own function:

| No. | Concept | Decided by | Asked as |
|---|---|---|---|
| 1 | **Ownership and access lists**: who may read, use, change or delete a stored thing | the `SecurityManager`, from `SecurityAttributes` (`owner_id`, `acl` naming actors, roles and groups) and the context's principal; `OwnershipSecurityManager::granted` | `Get*`, `Create*`, `Update*`, `Delete*`, `Write*`, `UseSkill`, `ReadDeleted`, `Restore`, and the acts in a conversation (`CancelChannel`, `WakeChannel`, `RetryMessage`, `AnswerQuestion`) |
| 2 | **Flow and clearance**: whether information may go where the act takes it, and who is cleared for it | the `ConfidentialityPolicy` (`ConfidentialityRules`), from the roles in the context, the classifications and the places | `may_publish`, `may_copy_information`, `cleared_for`, and their `require_*` forms; `require_classifiable` is validation |
| 3 | **Rules of particular actions**: what is neither ownership nor flow | a rule written into the policy for the variant: authorship, who answers for an agent, the system alone; or nothing, the action being a hook | `ModifyMessage`, `RevokeMessage`, `Reclassify`, `ApproveToolCall`, `ShowAgentCommand`, `ExecuteAgentCommand`, `ShowCommand`, `ExecuteCommand`, `Purge`; `CallTool`, `ReadSecret` |
| 4 | **Reach**: whether a thing can be reached from where one stands | reading it (1) followed by the flow (2): `view::channel_reading` for a channel - `GetChannel`, then for a user the clearance, for an agent the copy into its place -, `Get*` and `cleared_for` for a workspace, a skill, an artifact | no action of its own; answered "not found" |

## Which concept decides each action

From `OwnershipSecurityManager::check` (`caelumora-model/src/security.rs`). The system principal
is allowed everything before any variant is looked at. "Principal" is the context's principal - a
user themselves, an agent its owner -, "caller" the one acting - the agent itself.

| Actions | Rule | Concept |
|---|---|---|
| `GetPersonality`, `GetSessionTemplate`, `GetAgent`, `GetSession`, `GetChannel`, `GetComponents`, `GetUser`, `GetUserProfile`, `GetWorkspace`, `GetArtifact`, `GetSkill`, `GetGroup`, `UseSkill` | system-owned, or owned by the principal or the caller, or an entry grants `Read` to the principal or the caller (`Actor`, `Group`, `Role`) - the caller for an agent reading what it takes part in (issue 264) | 1 |
| `WriteWorkspace`, `WriteArtifact` | as reading, with `Write` | 1 |
| `CancelChannel`, `WakeChannel`, `RetryMessage`, `AnswerQuestion` | the principal reads the channel | 1: reading grants acting in the conversation |
| `ReadAgentTools` | the principal reads the agent or the channel | 1, two objects |
| `ApproveToolCall` | `answers_for`: the principal reads the agent; for an agent of the system, owns the channel | 3, built from 1 |
| `ShowAgentCommand`, `ExecuteAgentCommand` | `owners_only`: `answers_for`; else the principal reads the channel | 3, built from 1 |
| `ShowCommand`, `ExecuteCommand` | `owners_only`: the principal owns the channel; else reads it | 3, built from 1 |
| `Create*` (12) | the new entity's owner is the principal | 1 |
| `Update*`, `Delete*`, `ReadAgentContext`, `ReadAgentComponents`, `StopSession`, `ReadDeleted`, `Restore` | the principal owns it | 1 |
| `Reclassify` | a channel its owner; a message the channel's owner or its author (the caller); a note or memory entry its author. Whether the new classification may be given there is the flow's | 1 or 3 |
| `ModifyMessage`, `RevokeMessage` | the author is the caller | 3: authorship, not ownership |
| `Purge` | nobody but the system (issue 125) | 3 |
| `List*` (12) | always; the store filters each item with `Get*` | 1 per item |
| `CallTool`, `ReadSecret` | always: hooks for another policy (issue 24); a call is decided by the tools offered and the rules of its place, a secret has no owner yet (issue [310](../issues/310-secrets-have-no-owner.md)) | 3, decided elsewhere |

Most variants are decided by the principal; `ModifyMessage`, `RevokeMessage` and `Reclassify` of a
message, note or memory entry by the caller - an agent is judged as itself for what it said.
`Enforcement::require` logs a refusal with the principal either way. The flow is not the
`SecurityManager`'s at all (issue 304, reversing 138).

## The layers

| Layer | Asks | Form | Refusal |
|---|---|---|---|
| **Store** (`caelumora-model/src/store/access.rs`), rule 1 | `Get*` on every read; `List*`, then `Get*` per item as a filter; `Create*` with the new attributes; `Get*` then `Update*`/`Delete*` on the stored ones; `ReadDeleted` then `Restore`/`Purge` - always Read before the verb | `require`; `check` as the list filter | `NotFound` for what may not be read, `PermissionDenied` for what may not be changed, `Ok(false)` for what cannot be deleted |
| **Managers** (`SessionManager`, `SessionLoop`, `WorkspaceManager`, `ArtifactManager`, `SkillManager`, `GroupManager`, `SecretManager`), rules 2 and 4 | their verb before they act, where they act before they store; the flow of what they keep or make | `require`, `require_*` | the manager's error |
| **Channel loop** (`ChannelLoop`), rules 2 and 3 | what changes the channel, who is admitted, and what enters it (`admission`) | `require`, `require_*` | `ChannelError::Forbidden`, `NotCleared`; `MessageRejected` to the sender |
| **Owners of acts** (`ToolExecutor`, `CommandExecutor`, the agent carrying out a command, the approver, components on what they keep), rule 4 | the act's own question, once | `require` | the act's own answer: a forbidden result, `CommandFailed`, a left-out answer |
| **Views** (`caelumora-runtime/src/view`), rule 5 and reach | reaching (concept 4) and the flags of a snapshot | `may_*`; `require` for reading a member's tools, components and context | `*NotFound`, a flag false, `Forbidden` |
| **Filters** (event bus, command lists, store lists), rule 5 | the read question per item | `check` | silently left out |

The REST server asks nothing itself. It builds one context per request (`auth.rs`, without a place:
no decision reads the place from the context, the flow is given its places as arguments), reaches
a session through `open_session` and a channel through `channel_for`, reads through the views, and
maps refusals to `403`, every `*NotFound` to `404`.

## The map

Notation for whose name: **U** the user's own context; **T** a turn's (the agent as caller, its
owner as principal, whoever asked as initiator); **A** the agent's own (its owner as initiator);
**E** an envelope's, as its sender's actor set it; **V** a viewer's (U, or T with the place the
agent reads from); **C** the channel owner's (the channel's components); **S** the system's, which
asks nothing. After the name the concept. The rule column names the reason it is asked (R1-R5,
SECURITY.md); "store" is the store layer, in the context the caller passes.

### Sessions

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Find (`SessionManager::find`, behind open, stop, delete) | running: `GetSession` (U,1) by hand; stored: store `GetSession` | R1 | by the views, if the client navigated first (R5) | `NotFound` |
| Make from a template (`create_from_template`) | store `GetSessionTemplate`; a chosen workspace: `GetWorkspace`, `UpdateWorkspace` (U,1); participants resolved: store `GetUser`, `GetPersonality`, `CreateAgent` (U,1); store `GetUserProfile`; the template's workspace made (flow, then `CreateWorkspace`); then *Create*; then the workspace shared | R1; `UpdateWorkspace` R2 | `UpdateWorkspace` three times (attach, share, store) | `NotFound`, `PermissionDenied`, `WorkspaceOutOfScope`, `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `OutOfScope`; undone in S |
| Create (`create_session`) | per channel `require_classifiable`; `require_admitted`: per agent `GetPersonality` (A,1), scope, per member `require_cleared` with its roles (U,2); store `CreateSession`, per channel `CreateChannel` (U,1); groups in S | R1; the members: others | `GetPersonality` again as the channels are built | `ClassificationRefused`, `NotCleared`, `OutOfScope`, `PermissionDenied` |
| Open (`open_session`) | *Find*; channels loaded in S; per agent `GetPersonality` (A,1) - refused, the member is left out; components built in C or A (`GetComponents`, `ReadSecret`) | R1 | as creating | `NotFound` |
| Stop (`stop_session`) | *Find*; `StopSession` (U,1) | R4 | - | `NotFound`, `PermissionDenied` |
| Delete (`delete_session`) | *Find*; `DeleteSession` (U,1); stopped, channels in S; store `DeleteSession`; the scope in S | R2 + R1 | `DeleteSession` (pair) | `NotFound`, `PermissionDenied` |
| Restore (`restore_session`) | store `ReadDeleted` (U,1), as whoever restores it; store `ReadDeleted` + `Restore`; store `GetSession` | R1 | `ReadDeleted` (both store) | `NotFound`, `PermissionDenied` |
| Purge (`purge_session`) | store `ReadDeleted`; store `ReadDeleted` + `Purge`; channels and scope in S | R1 | - | `NotFound`, `PermissionDenied` |
| Rename, privacy, approval rules, description (`SessionLoop::update_*`) | store `GetSession` + `UpdateSession` (U,1) | R1 | - | `PermissionDenied` |
| Attach a workspace (`set_workspace`) | `UpdateSession` (U,1); `GetWorkspace`, `UpdateWorkspace` (U,1), scope; store `UpdateSession`; sharing it as *Share* of a workspace | R2 + R1 | `UpdateSession` (pair); `UpdateWorkspace` three times | `PermissionDenied`, `NotFound`, `WorkspaceOutOfScope` |
| Delete an agent (`delete_agent`) | store `GetAgent`; `DeleteAgent` (U,1); it leaves its channels in S; store `DeleteAgent` | R2 + R1 | `DeleteAgent` (pair) | `NotFound`, `PermissionDenied` |
| Restore, purge an agent | store only | R1 | - | `NotFound`, `PermissionDenied` |

### Channels

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Reach a channel to act in it (`SessionLoop::channel_for`, the REST server's way) | `channel_reading`: `GetChannel` (U,1), `cleared_for` (U,2) | reach | by the views if the client navigated | `ChannelNotFound` for absent, refused and not cleared alike |
| Create (`SessionLoop::create_channel`) | `UpdateSession` (U,1); `require_classifiable`; store `GetUserProfile`; members of the session; `require_admitted` (A `GetPersonality`, scope, U cleared with the member's roles); store `CreateChannel` (U,1); group in S; store `UpdateSession` | R2 + R1 | `UpdateSession` (pair); `GetPersonality` as built | `PermissionDenied`, `ClassificationRefused`, `NotCleared`, `OutOfScope`, `NameTaken` |
| Delete (`SessionLoop::delete_channel`) | `GetChannel` (U,1); `DeleteChannel` (U,1); store `UpdateSession` as the session drops it; store `GetChannel` + `DeleteChannel`; actors stopped; the scope in S | R2 + R1 | `GetChannel`, `DeleteChannel` (pair) | `ChannelNotFound`, `PermissionDenied` |
| Restore (`SessionLoop::restore_channel`) | `UpdateSession` (U,1); store `ReadDeleted`; `require_admitted` as creating; store `ReadDeleted` + `Restore`; store `UpdateSession` | R2 + R1 | `UpdateSession` (pair), `ReadDeleted` (store) | `PermissionDenied`, `ChannelNotFound`, `NotCleared`, `NameTaken` |
| Rename, language, description, workspace switch, approval rules (`ChannelLoop::update_*`) | store `GetChannel` + `UpdateChannel` (U,1) | R1 | - | `PermissionDenied` |
| Reclassify (`ChannelLoop::reclassify`) | `Reclassify` of the channel (U,1); `require_classifiable`; per member `require_cleared` (U,2); store `UpdateChannel`; `may_publish` of the old into the new to learn whether it was lowered | R2 + R1; the last a question | - | `Forbidden`, `ClassificationRefused`, `NotCleared` |
| Add a participant (`add_participant`) | `UpdateChannel` (U,1); structural; `GetPersonality` (A,1); scope; `require_cleared` with the member's roles (U,2); store `UpdateChannel`; the actor's components (A) | R2 + R1 | `UpdateChannel` (pair) | `Forbidden`, `PermissionDenied`, `OutOfScope`, `NotCleared` |
| Remove a participant (`remove_participant`) | structural (`ApproverStays`, `NotAParticipant`); store `GetChannel` + `UpdateChannel` (U,1) | R1 | - | `PermissionDenied` |
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
| `RetryRequested` | `RetryMessage` (E,1); a message an agent said, not taken back, or an agent taking part | `MessageNotFound`, `Forbidden`, `MessageUnchangeable`, `Invalid` |
| `QuestionAnswered`, `QuestionDeclined` | `AnswerQuestion` (E,1); whether its sender was asked is the agent's | `Forbidden` |
| `CommandRequested`, `NudgeRequested` | form only: from a user, naming someone taking part; a command is the agent's to admit | `Invalid` |
| everything else | nothing; an approval is the approver's | - |

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
| An approval by click (`ApproverActor::handle_response`) | `ApproveToolCall` (E,3) | R4 | `approvers_of` for the strategy (R5) | the answer left out |
| An approval by a person's rule (`UserActor::answer_by_rule`) | `may_approve` (U,3), then as a click | R5 | up to three times per answer | the rule does not answer |
| A command of the session (`CommandExecutor::execute`) | `ShowCommand`, `ExecuteCommand` with the channel (U,3) | R4 | `/wakeup` asks `WakeChannel` itself | `Unknown`, `Forbidden` |
| Commands shown (`visible_by`, `/help`, completion) | per command `ShowCommand` (U,3) | R5 | - | left out |
| A command of an agent asked for (`UserActor::request_command`) | whom it names: the agents' commands as the issuer sees them (`ShowAgentCommand`, U,3) | R5 | by the agent | `Unknown`, `InvalidArguments` |
| An agent carries a command out (`AgentCommands::carry_out`) | `ShowAgentCommand`, `ExecuteAgentCommand` (E,3) | R4 | - | `CommandFailed` |
| A secret (`SecretManager::resolve`) | `ReadSecret` (3) - T at a call, A or C as a component is built, S for a storage | R4 | - | `SecretError::PermissionDenied` |

### What an agent reads beyond its channel

An agent's views are built per piece of work, with the agent's place; all in **T**.

| Operation | Questions in order | Rule | Refusal |
|---|---|---|---|
| Its own session (`ViewsFactory::session`) | `may_read_session` (T,1) - by the group of the session's participants | reach | no session |
| A channel by name or id (`SessionView::channel`, `channel_named`; `read_history`, `read_notes`) | `channel_reading`: `GetChannel` (T,1), `may_copy_information` of the channel into the place (T,2) - as a whole or entry by entry; entry by entry `may_copy_from` per message or note | reach | `ChannelNotFound`, `NoChannelNamed` alike |
| All channels (`channels`, `all_channels`) | as above per channel | reach | left out |
| Its own private notes of other channels (`own_private_notes`) | no `GetChannel` - its own knowledge -; per note `may_copy_from` (T,2) | reach | left out |
| Its memory (`AgentMemory::recalled`, `find`, `update`, `forget`) | per entry `may_copy_information` of its origin, by the origin's privacy then and now | R4: the agent's own knowledge, as its private notes | left out |
| Reclassify a note (`AgentNotes::reclassify`) | the note is there; `Reclassify` of the note (T,3); `require_publish` of the new classification (T,2) | R4 | `InvalidParameter`, `PermissionDenied` |
| Reclassify a memory entry (`AgentMemory::reclassify`) | the entry recalled; `Reclassify` (T,3); `require_publish` (T,2) | R4 | as above |

### Skills

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| A user lists or reads skills (`Views::skills`, `skill`) | store `GetSkill` (V,1); `cleared_for` (V,2) | R1 + reach | - | left out; `SkillNotFound` |
| An agent's catalogue, `list_skills`, `load_skill`, `/skills`, `/skill`, compaction (`Views::usable_skills`, `usable_skill_named`) | store `GetSkill` of what is seen from the place (T,1); `UseSkill` (T,1); `cleared_for` (T,2); `may_publish` into the place (T,2) | reach | - | left out; `NoSkillNamed` |
| Create (`SkillManager::create`) | `require_classifiable`; `require_cleared_for` (U or T,2); store `CreateSkill` (1) | R4 (flow) + R1 | - | `ClassificationRefused`, `NotCleared`, `PermissionDenied`, `NameTaken` |
| Change, reclassify (`SkillManager::update`) | store `GetSkill`; for a classification as creating; store `GetSkill` + `UpdateSkill` | R1 (+ flow) | `GetSkill` (store) | `NotFound`, `ClassificationRefused`, `NotCleared`, `PermissionDenied` |
| Delete, restore, purge | store only | R1 | - | `PermissionDenied`, `NotFound` |

### Workspaces

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| A user lists or reaches one (`Views::workspaces`, `workspace`) | store `GetWorkspace` (V,1); `cleared_for` (V,2) | R1 + reach | by `reached` for a file route | left out; `WorkspaceNotFound` |
| A user works in it directly (`WorkspaceManager::reached`, the file routes) | store `GetWorkspace` (U,1); `cleared_for` (U,2), else `NotFound`; for a change `WriteWorkspace` (U,1) | reach, then R4 | - | `NotFound`, `ReadOnly` |
| In a channel (`usable_in_channel`, behind `terminal_in`, the terminal and artifact tools, the workspace prompt) | session and channel read in S; `may_read_session` (1); `channel_reading`; `usable_in`: the channel uses it, the caller takes part, scope, `may_read_workspace` (1), `may_publish` both ways (2); for a change `WriteWorkspace` (1) | reach, then R4 | - | `NotUsableInChannel`, `ReadOnly` |
| A running command goes on (`may_run`, as the channel's members or the workspace change) | the chain of *In a channel* per current member | an act that lasts, asked again as what permitted it changes | per member and event | the command ended |
| The channel's workspace offered (`ChannelView::workspace`) | `usable_in` (V) | R5 | - | none |
| Create (`WorkspaceManager::create`) | `require_classifiable`; `require_publish` into the storage's destination (U,2); `require_cleared_for` (U,2); store `CreateWorkspace` (U,1) | R4 (flow) + R1 | - | `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `PermissionDenied`, `NameTaken` |
| Change, reclassify (`update`) | store `GetWorkspace`; a classification as creating; store `GetWorkspace` + `UpdateWorkspace` | R1 (+ flow) | `GetWorkspace` (store) | `NotFound`, `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `PermissionDenied` |
| Share (`set_access`, `share`) | store `GetWorkspace`; `UpdateWorkspace` (U,1); `require_nameable`: store `GetGroup` per newly named group; store `UpdateWorkspace` | R2 + R1 | `UpdateWorkspace` (pair) | `NotFound`, `PermissionDenied`, `Validation` |
| Delete, restore | store only | R1 | - | `PermissionDenied`, `NotFound` |
| Purge (`purge`) | store `ReadDeleted`; `Purge` (U,3); files removed; store `ReadDeleted` + `Purge` | R2 + R1 | `Purge` (pair) | `PermissionDenied` |

### Artifacts

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| List, reach (`Views::artifacts`, `artifact`, `ChannelView::artifacts`) | read in S; `may_read_artifact` with the artifact's list, else per version with the version's (V,1); `cleared_for` (V,2) | reach | - | left out; `ArtifactNotFound` |
| Where a version was published (`ArtifactView::publication`) | `Views::channel_of`: the session, then `channel_reading`; gone, as a live one would be reached | reach | - | not named |
| Publish (`ArtifactManager::publish`, the artifact tool) | the workspace's reach (*In a channel*); an existing artifact found in S: `WriteArtifact` (T,1) - one not the caller's to add to is refused as such, not taken for a name to make anew; or `CreateArtifact` (T,1); `require_publish` into the storage's destination (T,2); content kept; store `CreateArtifact` for a new one | R2 + R1 | `CreateArtifact` (pair) | `PermissionDenied`, `ClassifiedOtherwise`, `StorageNotCleared` |
| Share (`set_access`) | store `GetArtifact`; `UpdateArtifact` (U,1); `require_nameable`; store `UpdateArtifact` | R2 + R1 | `UpdateArtifact` (pair) | `NotFound`, `PermissionDenied`, `Validation` |
| Keep in a scope (`keep`) | store `GetArtifact`; `UpdateArtifact` (U,1), before the scope is compared with the owner; store `UpdateArtifact` | R2 (it looks at the owner first) + R1 | `UpdateArtifact` (pair) | `NotFound`, `PermissionDenied`, `InvalidScope` |
| Describe (`describe`) | store `GetArtifact`; store `GetArtifact` + `UpdateArtifact` | R1 | - | `NotFound`, `PermissionDenied`, `Invalid` |
| Delete, restore | store only | R1 | - | `PermissionDenied`, `NotFound` |
| Purge (`purge`) | store `ReadDeleted`; `Purge` (U,3); content removed; store `ReadDeleted` + `Purge` | R2 + R1 | `Purge` (pair) | `PermissionDenied` |

### Groups

| Operation | Questions in order | Rule | Refusal |
|---|---|---|---|
| List, reach (`Views::groups`, `group`) | store `GetGroup` (V,1); the participants' groups left out of the list | R1 | left out; `GroupNotFound` |
| Make a team (`GroupManager::create`) | store `CreateGroup` (U,1) | R1 | `PermissionDenied`, `NameTaken` |
| Set members (`set_members`) | the users read in S (`UnknownMember`); then as renaming | R1 | `UnknownMember`, then as below |
| Rename (`update`) | store `GetGroup`; a participants' group refused (structural); store `UpdateGroup` | R1 | `NotFound`, `Participants`, `PermissionDenied` |
| Delete, restore, purge | store; a participants' group refused | R1 | `NotFound`, `Participants`, `PermissionDenied` |
| Name a group in an access list (`require_nameable`) | store `GetGroup` (U,1) per newly named group | R1 | `Validation` |
| The participants' groups (`keep_participants`, `purge_participants`) | in S, after the change of the participants was permitted | - | - |

### Views, snapshots and events

| Operation | Questions | Rule | Refusal |
|---|---|---|---|
| Sessions (`Views::sessions`, `session`) | store `GetSession`; running, `may_read_session` (V,1) | R1 + reach | left out; `SessionNotFound` |
| A deleted session or channel (`gone_session`, `gone_channel`) | read in S; `may_read_session`; a channel as a live one is reached (`SessionView::readable`, the clearance included) | reach | `Never` |
| Channels (`SessionView::readable`) | `channel_reading` (V) | reach | `ChannelNotFound` |
| Flags of a session, channel, member, workspace, skill, group, artifact | `may_change_session`, `may_change_channel`, `may_read_agent_context`, `may_read_agent_tools`, `may_read_agent_components`, `may_approve`, `may_retry_message`, `may_cancel_channel`, `may_delete_agent`, the member's commands, `may_change_workspace`, `may_write_workspace`, `may_change_skill`, `may_change_group`, `may_change_artifact`, `may_write_artifact` | R5 | a flag false |
| A member's tools, components, context, private notes (`tool_sets`, `components`, `context`) | `ReadAgentTools`, `ReadAgentComponents`, `ReadAgentContext` (V,1), the context also only of a channel read whole | R4: reading them is the views' act | `Forbidden` |
| What was deleted (`deleted_*`) | store `ReadDeleted` per item; flags `may_restore`, `may_purge` | R1 + R5 | - |
| An event reaching a subscriber (`EventBus`, `subscribe_as`) | per event and party the read question: `GetChannel` for a channel's events, its members' and an agent's state in it; `GetAgent`, `GetUser`, `GetUserProfile`, `GetWorkspace`, `GetSkill`, `GetGroup`, `GetArtifact` by kind; `GetSession` for a session's | R5 | not delivered |

### Users

| Operation | Questions | Rule | Refusal |
|---|---|---|---|
| A profile read or changed, approval rules (`UserController`) | store `GetUserProfile`, `UpdateUserProfile` (U,1) | R1 | `NotFound`, `PermissionDenied` |
| Users, agents, personalities, profiles read by the runtime (controllers, names in the views) | in S | - | - |
| Templates listed | store `GetSessionTemplate` per item (U,1) | R1 | left out |
| Roles, limiting roles | no question: carried by the context, put there by the `UserController` and the `AgentController` from the roles as they are now | - | - |

## Findings

### Decided to stay

- **Pairs (rule 2).** An operation that does something before it stores asks its verb, and the
  store asks again: deleting a session, a channel or an agent, sharing, purging, keeping an
  artifact. No token tells the store it was asked already.
- **The agents' own flow.** An agent asks the flow of what it gives up or brings in its own name -
  its turn still flows into the channel (`AgentActor::turn_flow`), an external agent's remote side,
  what a request brings, a lowered channel (`ExternalAgentActor`). That is the agent's act of
  withholding, not a pre-check of the channel's, which asks again as anything enters it (rule 3).
  Likewise a tool's result is asked into the agent's context before the channel asks it into
  itself: two destinations.
- **Memory.** An agent recalls its memory by the flow of each entry's origin itself - its own
  knowledge, reached through itself as its private notes of other channels are. One enforcement for
  every component's state is issue [65](../issues/65-generic-component-state.md).
- **The reach is one function for offering and acting.** `channel_reading` and
  `WorkspaceManager::usable_in` are built from `may_*` and answer with an option: the views turn
  `None` into "not there", an operation into "not found". It is neither enforcing nor offering, but
  the one way to reach a thing, so that both see the same.
- **A refusal that is no error.** The `ToolExecutor` and the approver ask `require` with a refusal
  of `()` and turn it into their own outcome - a forbidden result, an answer left out -, so that the
  refusal is logged as every act's is.
- **Filters with `check`.** The event bus, the command lists and the store's list filter ask the raw
  question, since they take any action.
- **Lowered.** `ChannelLoop::reclassify` asks `may_publish` of the old classification into the new
  one to learn whether the channel was lowered - the question an agent asks at its next step, so
  that both agree.
- **Running commands.** A command in a workspace lasts; whether it may go on is asked again, in
  every member's name, whenever the members or the workspace change (`may_run`).
- **A new version of an artifact.** `publish` asks `WriteArtifact` of an artifact found in the
  runtime's name, so that one not the caller's to add to is refused as such rather than taken for a
  name to make anew.
- **`GetPersonality` twice.** An agent's personality is read as the member is admitted and again as
  the channel is built: two acts.
- **The place in a REST context.** No decision reads it; the flow is given its places as arguments.
- **`CallTool` and `ReadSecret`** stay hooks for another policy (issue 24); secrets get an owner in
  issue [310](../issues/310-secrets-have-no-owner.md).

### Open

In issue [312](../issues/312-order-and-reach-not-everywhere.md):

- creating a session or a channel, and restoring one, asks the others before its own verb;
- the workspace and skill managers ask the flow and validation before the store asks the verb;
  the group manager answers structural refusals before the group is read;
- the managers reach workspaces, skills and artifacts by id without the clearance the views ask;
  deleting and restoring a channel reach it without the clearance;
- the event bus delivers without the clearance;
- a refused approval is not told to its sender;
- a note's or memory entry's reclassification carries no attributes; a channel's components are
  built in its owner's name;
- attaching and sharing a workspace asks `UpdateWorkspace` three times, an approval by a rule
  `ApproveToolCall` up to three times.
