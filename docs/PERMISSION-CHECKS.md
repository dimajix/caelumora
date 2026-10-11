# Permission Checks

This document is the map of every place where the runtime asks the `SecurityManager` a permission
question: which operation asks, in whose name, which `ActionType`, which of the four concepts of
permission decides it, and where the same question is asked again on the way. It records what the
code does, as of 2026-10-11 (issue [304](../issues/304-review-of-the-permission-concept.md)); the
rules themselves are in [SECURITY.md](SECURITY.md) and [CONFIDENTIALITY.md](CONFIDENTIALITY.md),
and what each action is expected to ask is asserted by the security suite
(`caelumora-runtime/tests/security_suite.rs`, issue 198). A new action, or a change to what one
asks, changes this map as it changes the suite.

## The four concepts

Who may do what is decided by four concepts, each with its own data, its own rule and its own
places in the code:

| No. | Concept | Decided by | Asked as |
|---|---|---|---|
| 1 | **Ownership and access lists**: who may read, use, change or delete a stored thing | `SecurityAttributes` (`owner_id`, `acl`) and the context's principal; `OwnershipSecurityManager::granted` | `Get*`, `Create*`, `Update*`, `Delete*`, `Write*`, `Use*`, `ReadDeleted`, `Restore`, and the acts in a conversation (`CancelChannel`, `WakeChannel`, `RetryMessage`, `AnswerQuestion`) |
| 2 | **Flow rules and clearances**: whether information may go where the action takes it, and who is cleared for it | `ConfidentialityRules`: `flows_to`, `may_publish`, `may_copy`, `may_classify`, `cleared` | `Publish`, `CopyInformation`, `Classify`, `Reclassify`, `JoinChannel` |
| 3 | **Rules of particular actions**: what is neither ownership nor flow | a rule written into the policy for the variant: authorship (`ModifyMessage`), who answers for an agent (`ApproveToolCall`, `*AgentCommand`), the system alone (`Purge`); or nothing, the action being governed outside the policy | `ModifyMessage`, `RevokeMessage`, `ApproveToolCall`, `ShowAgentCommand`, `ExecuteAgentCommand`, `Purge`; `CallTool`, `ReadSecret`, `ShowCommand`, `ExecuteCommand` |
| 4 | **Reachability**: whether a thing can be reached at all from where one stands | three mechanisms: `MemberOf` entries of an access list (matched against the principal and, for reading, the caller - a member reaches what their session or channel shares); the navigation of the views, with `channel_reading`; `SessionPrivacy::permits` inside `CopyInformation` | no action of its own: it is `GetSession`/`GetChannel` answered "not found", the `MemberOf` subject, and the privacy half of `CopyInformation` |

Concept 4 is the one without a home: it is partly ownership (the `MemberOf` subject makes
membership an access-list fact), partly confidentiality (privacy is asked inside the flow
question), and partly a convention of the views and of the operations that repeat what the views
ask (`channel_reading`) before they act in a channel.

## Which concept decides each action

From `OwnershipSecurityManager::check` (`caelumora-model/src/security.rs`). The system principal
is allowed everything before any variant is looked at. "principal" is the context's principal - a
user themselves, an agent its owner -, "caller" the one acting - the agent itself.

| Actions | Rule | Concept |
|---|---|---|
| `GetPersonality`, `GetSessionTemplate`, `GetAgent`, `GetSession`, `GetChannel`, `GetComponents`, `GetUser`, `GetUserProfile`, `GetWorkspace`, `GetArtifact`, `GetSkill`, `UseSkill` | system-owned, or owned by the principal or the caller, or an entry grants `Read` to the principal or the caller (`Actor`, `MemberOf`, `Role`) | 1, with 4 inside: the caller-as-member rule (issue 264) and `MemberOf` |
| `WriteWorkspace`, `WriteArtifact` | as reading, with `Write` | 1 (+4) |
| `CancelChannel`, `WakeChannel`, `RetryMessage`, `AnswerQuestion` | the principal reads the channel | 1: reading grants acting in the conversation |
| `ReadAgentTools` | the principal reads the agent or the channel | 1, two objects |
| `ApproveToolCall` | `answers_for`: the principal reads the agent; for an agent of the system, owns the channel | 3, built from 1 |
| `ShowAgentCommand`, `ExecuteAgentCommand` | `owners_only`: `answers_for`; else the principal reads the channel | 3, built from 1 |
| `Create*` (11) | the new entity's owner is the principal | 1 |
| `Update*`, `Delete*` (22), `ReadAgentContext`, `ReadAgentComponents`, `StopSession`, `ReadDeleted`, `Restore` | the principal owns it | 1 |
| `Purge` | nobody but the system (issue 125) | 3 |
| `List*` (11) | always; the store filters each item with `Get*` | 1 per item |
| `ShowCommand`, `ExecuteCommand`, `CallTool`, `ReadSecret` | always: governed outside the policy - a command's scope, the approval strategy and the tool rules of a place, the secret policy | 3, decided elsewhere |
| `CopyInformation` | same channel; or same session or the source's privacy permits the reader's, and the entry's classification flows into the reader's | 2 + 4 (privacy) |
| `Classify` | the compartments are configured | 2 |
| `Reclassify` | a channel: the principal owns it, and `may_classify`; a message: the principal owns the channel or the caller is the author, and the new classification flows into the channel; a note or memory entry: the caller is the author, and it flows | 1 or 3, + 2 |
| `Publish` | the actor is the caller, and the classification flows into the channel | 3 (identity) + 2 |
| `ModifyMessage`, `RevokeMessage` | the author is the caller | 3: authorship, not ownership |
| `JoinChannel` | the roles given clear the classification | 2; the roles are looked up by the runtime (issue 181) |

Most variants are decided by the principal; `Publish`, `ModifyMessage`, `RevokeMessage` and
`Reclassify` of a message, note or memory entry by the caller - an agent is judged as itself for
what it said. `Enforcement::require` logs a refusal with the principal either way.

## Who asks

Seven kinds of code ask, each in its own way and with its own refusal:

| Layer | Asks | How | Refusal |
|---|---|---|---|
| **Stores** (`caelumora-model/src/store/access.rs`) | `Get*` on every read (with the stored attributes), `List*` then `Get*` per item, `Create*` with the new attributes, `Get*` then `Update*`/`Delete*` with the stored ones, `ReadDeleted` then `Restore`/`Purge` | `Enforcement::require`; `check` as the filter of a list | `NotFound` for what may not be read, `PermissionDenied` for what may not be changed, `Ok(false)` for what cannot be deleted |
| **Managers** (`SessionManager`, `SessionLoop`, `ArtifactManager`, `WorkspaceManager`, `SkillManager`, `SecretManager`) | the operation's own action before acting (`StopSession`, `DeleteSession`, `DeleteAgent`, `UpdateSession`, `Classify`, `UpdateWorkspace`, `Purge`, `WriteArtifact`, `ReadSecret`, ...), then the store asks again when it stores | `require`, `may_*` | the manager's error; `StorageNotCleared`, `NotCleared`, `ReadOnly`, `ClassificationRefused` |
| **Channel loop** (`ChannelLoop`) | what changes the channel (`UpdateChannel`, `Reclassify`, `CancelChannel`, `WakeChannel`), who is admitted (`GetPersonality` in the agent's name, `JoinChannel`), and what enters it: `Publish` per part of every envelope, `ModifyMessage`, `RevokeMessage`, `Reclassify`, `RetryMessage` in the envelope's context | `require`; `may_*` in `admits`/`admits_change` | `ChannelError::Forbidden`, `NotCleared`; an envelope not admitted is dropped with a warning |
| **Actors** (`UserActor`, `ApproverActor`, the agents) | before sending: `Publish`, `ModifyMessage`, `RevokeMessage`, `Reclassify`, `RetryMessage`, `AnswerQuestion`, `ApproveToolCall` (rule answers), the commands; an agent `Publish` for what it brings, says and gives up | `require`, `may_*` | `ChannelError::Forbidden`, `ClassificationRefused`; a turn given up as lowered |
| **Views** (`caelumora-runtime/src/view`) | reaching: `GetSession`, `channel_reading` (`GetChannel`, then `CopyInformation` for an agent or `JoinChannel` for a user), `GetWorkspace`/`GetSkill`/`GetArtifact` plus `JoinChannel` for the classification; offering: `may_change`, `may_read_context`, `may_approve`, `may_retry`, `may_cancel`, `may_delete`, `may_restore`, `may_purge` as flags of a snapshot | `may_*`; `require` for `tool_sets`, `components`, `context` | `*NotFound` for what is not reached, `Forbidden` for the three operations on a member |
| **Components and tools** (`ToolExecutor`, the components reading across channels, skills, workspace tools) | `CallTool` when a call is admitted and again when it runs, `Publish` for every result, for a skill, for a tool set's destination; `CopyInformation` per entry read elsewhere; `UseSkill`; `Reclassify` of notes and memory | `may_*`, `require`, through the views | a tool result `Forbidden`/`Withheld`, `ToolError::PermissionDenied`, an entry left out |
| **Filters** (event bus, command lists, store lists) | `Get*` per event for every party of `subscribe_as`; `ShowCommand`/`ShowAgentCommand` per command; `Get*` per listed item | `check` | silently left out |

The REST server asks nothing: it builds one `SecurityContext` per request (`auth.rs`, with no
session and no channel) and maps `ViewError::Forbidden` and `StoreError::PermissionDenied` to 403
and every `*NotFound` to 404.

## The map

Notation for whose name: **U** the user's own context; **T** the turn's (the agent as caller, its
owner as principal, whoever asked as initiator); **A** the agent's own (its owner as initiator);
**E** the envelope's, as its sender's actor set it; **V** the viewer's (a user's own, or **T** with
the place the agent reads from); **S** the system's, which asks nothing. The concept is given per
question; "again" names where the same question is asked once more on the same path.

### Sessions

| Operation | Questions in order (whose name, concept) | Again | Refusal |
|---|---|---|---|
| Make a session from a template (`SessionManager::create_from_template`) | `GetSessionTemplate` (U, 1), `GetWorkspace` + `UpdateWorkspace` to attach one (U, 1), `GetUser`/`ListAgents`/`GetPersonality`/`CreateAgent` while participants are resolved (U, 1), `GetUserProfile` (U, 1); per channel `Classify` (U, 2), per participant `GetPersonality` (A, 1) and `JoinChannel` with the member's roles (U, 2); `CreateSession`, `CreateChannel` (U, 1) | `GetPersonality` again when the channel is built (`ChannelLoop::new`, A) | `NotFound`, `ClassificationRefused`, `NotCleared`, `PermissionDenied`; an undo in S |
| Open (`open_session`) | `GetSession` (U, 1); channels loaded in S; per member `GetPersonality` (A, 1) - a refusal leaves the member out with a warning | `GetSession` again if the client first read the session through the views | `NotFound` |
| Stop, delete (`stop_session`, `delete_session`) | `GetSession` then `StopSession`/`DeleteSession` (U, 1) | `DeleteSession` again by the store; what lies in the scope is deleted in S | `NotFound`, `PermissionDenied` |
| Restore, purge (`restore_session`, `purge_session`) | the deleted session is read in S for its deletion time; `ReadDeleted` then `Restore`/`Purge` (U, 1) by the store; `GetSession` (U, 1) after restoring | - | `NotFound`, `PermissionDenied` |
| Rename, attach a workspace (`set_workspace`) | `UpdateSession` (U, 1); `GetWorkspace` + `UpdateWorkspace` (U, 1) | `UpdateSession` again by the store | `PermissionDenied` |
| Delete an agent (`delete_agent`) | `GetAgent`, `DeleteAgent` (U, 1); it leaves its channels in S | `DeleteAgent` again by the store | `NotFound`, `PermissionDenied` |

### Channels

| Operation | Questions in order (whose name, concept) | Again | Refusal |
|---|---|---|---|
| Reach a channel to act in it (`SessionLoop::channel_for`) | `GetChannel` (U, 1+4), `JoinChannel` with the principal's own roles (U, 2) - `require_seen`, the user half of `channel_reading` written by hand | the views asked the same when the client listed it | `ChannelNotFound` for both |
| Create (`create_channel`) | `UpdateSession` (U, 1); `Classify` (U, 2); `GetUserProfile` (U, 1); per participant `GetPersonality` (A, 1), scope, `JoinChannel` with the member's roles (U, 2); `CreateChannel` (U, 1) | `UpdateSession` again by the store as the session takes the channel; `GetPersonality` again when it is built | `PermissionDenied`, `ClassificationRefused`, `NotCleared` |
| Delete (`delete_channel`) | `UpdateSession` by the store as the session drops it (U, 1); `GetChannel`, `DeleteChannel` by the store (U, 1) - a refusal here is only warned, the channel is taken out of the session all the same | - | `PermissionDenied` |
| Restore (`restore_channel`) | `UpdateSession` (U, 1); `ReadDeleted` (U, 1); per participant as creating; `ReadDeleted` + `Restore` by the store | `UpdateSession` again by the store | as creating |
| Reclassify (`ChannelLoop::reclassify`) | `Reclassify` of the channel (U, 1+2); per current member `JoinChannel` against the new classification (U, 2); `GetChannel` + `UpdateChannel` by the store (U, 1); `Publish` of the old into the new only to learn whether it was lowered, not as a permission | - | `Forbidden`, `NotCleared` |
| Add a participant (`add_participant`) | `UpdateChannel` (U, 1); `GetPersonality` (A, 1); scope; `JoinChannel` with the member's roles (U, 2); `GetChannel` + `UpdateChannel` by the store | `UpdateChannel` again by the store | `Forbidden`, `PermissionDenied`, `NotCleared` |
| Remove a participant | `GetChannel` + `UpdateChannel` by the store (U, 1) | - | `PermissionDenied` |
| Cancel, wake (`cancel`, `cancel_actor`, `interrupt_tool_call`, `wakeup`, `wakeup_actor`) | `CancelChannel` / `WakeChannel` (U, 1) | - | `Forbidden` |

### What enters a channel

Every envelope an actor sends is admitted by the channel in the envelope's context - the second
line, so that no actor can act in another's name (issue 228). The first line is the actor's own.

| Operation | First line (actor) | Second line (channel, E) | Refusal |
|---|---|---|---|
| Send a message (`UserActor::send_message`) | `Publish` (U, 2+3) | `Publish` per part (`admits`) | `ClassificationRefused`; dropped |
| Modify, take back (`modify_message`, `revoke_message`) | `ModifyMessage`/`RevokeMessage` of the author (U, 3), then `Publish` for a modification (U, 2) | the same (`admits_change`), then `Publish` | `Forbidden`; dropped |
| Reclassify a message (`reclassify_message`) | `Reclassify` (U, 1 or 3, + 2) | the same | `Forbidden`; dropped |
| Ask for another answer (`request_retry`, `request_retry_of_turn`) | `RetryMessage` (U, 1) | the same | `Forbidden`; dropped |
| Answer an agent's questions (`answer_questions`) | `AnswerQuestion` (U, 1) | nothing: whether the one answering was asked is the agent's check | `Forbidden` |
| Nudge (`request_response`) | nothing | nothing but that a user nudges a participant | dropped |
| Wake (`/wakeup`) | nothing in the actor; `WakeChannel` in the loop (U, 1) | - | `Forbidden` |
| An agent's answer, tool results, what it gives up (`ToolExecutor::admitted`, `AgentActor::turn_flow`, `ExternalAgentActor::lowered`, `exchange`) | `Publish` per tool result and for the turn's label (T, 2+3); an external agent once when it is built (A) and per request (T) | `Publish` per part (`admits`) | a result `Withheld`; a turn given up as lowered; dropped |

### Tool calls, approvals, commands

| Operation | Questions in order (whose name, concept) | Again | Refusal |
|---|---|---|---|
| A tool call (`ToolExecutor::admit`, `check`) | offered?; `CallTool` (T, 3 - answered yes by the policy, decided by the place's rules); the place rule; approval; `CallTool` again when it runs (T) | `CallTool` twice | a result `Forbidden`; `PermissionDenied` |
| A tool set with a destination (`tool_sets`) | `Publish` of the channel's classification into the destination (T, 2) | - | the set offered as withheld |
| Who may approve (`ApproverActor::approvers_of`) | `ApproveToolCall` per user participant, in each one's own context (U, 3) - on every request, answer and membership change | - | left out; nobody → denied at once |
| An approval answered (`send_approval`, `handle_response`) | a click: nothing in the user's actor; `ApproveToolCall` by the approver (E, 3), then `approvers_of` again for the strategy. A rule's answer: `ApproveToolCall` in the user's actor first (U, 3), then as a click | up to three times per answer | the answer left out with a warning |
| A user command (`UserActor::execute_command`, `visible_commands`, `/help`) | `ShowCommand` as a filter (U, 3 - always yes), `ShowCommand` then `ExecuteCommand` (U, 3) | - | `Unknown`, `Forbidden` |
| An agent command (`request_command` → `AgentCommands`) | `ShowAgentCommand` as a filter (U, 3); `ShowAgentCommand` + `ExecuteAgentCommand` to admit (U, 3); the same again when the agent carries it out, in the request's context (E = U, 3); the channel checks only that a user asks a participant | three times | `Unknown`, `Forbidden`; `CommandFailed` |
| A secret (`SecretManager::resolve`) | `ReadSecret` (T at a call, A at the build of a component, 3 - always yes, decided by the secret policy) | - | `PermissionDenied` |

### What an agent reads beyond its channel

An agent's own session is handed to it without a question (`ViewsFactory::session`): it takes part.
From there, every channel and every entry is asked as copying it into the agent's place - its
channel, classified as its work is now - in the turn's context (concept 2 + 4), after
`GetChannel` (1 + 4) where it reaches a channel at all.

| Operation | Questions in order (whose name, concept) | Refusal |
|---|---|---|
| A channel by name or id (`SessionView::channel`, `channel_named`; `read_history`, `read_notes <name>`) | `GetChannel` (T, 1+4); `CopyInformation` of the channel as a whole into the agent's place (T, 2+4) → whole or entry by entry; then per message or note `CopyInformation` of the entry when entry by entry | `NoChannelNamed`, `ChannelNotFound` for absent and refused alike; nothing released → the same |
| All channels (`SessionView::channels`; `read_notes` without a name) | the same per channel; only those readable as a whole are listed | left out |
| Its own private notes of other channels (`own_private_notes`) | `CopyInformation` per note (T, 2+4), no `GetChannel`: what it knows itself | left out |
| Its memory (`AgentMemory::may_recall`; the tools) | `CopyInformation` per entry (T, 2+4), with the origin's privacy as written and as it is now; no `GetChannel` | left out; "no entry you remember here" |
| Reclassify a note, a private note, a memory entry | `Reclassify` of a note or memory entry, author and flow only (T, 3+2) | `PermissionDenied` |
| Its own channel for the prompt (`session.channel(own)`) | `GetChannel` (T); `CopyInformation` into itself answers yes without asking | others' notes omitted |

### Skills

| Operation | Questions in order (whose name, concept) | Refusal |
|---|---|---|
| A user lists or reads skills (`Views::skills`, `skill`) | `ListSkills` + `GetSkill` per item by the store (V, 1); `JoinChannel` for the skill's classification with the user's roles (V, 2) | left out; `SkillNotFound` |
| An agent lists, loads, updates a skill (`usable_skills`, `usable_skill_named`) | `GetSkill` by the store (T, 1); `UseSkill` (T, 1); `Publish` of the skill into the place (T, 2) - no clearance | left out; `NoSkillNamed` |
| Create, reclassify (`SkillManager`) | `Classify` (T/U, 2); `JoinChannel` with the principal's roles (2); `CreateSkill`/`GetSkill` + `UpdateSkill` by the store (1) | `ClassificationRefused`, `NotCleared`, `PermissionDenied` |
| Purge | `ReadDeleted` by the store, `Purge` (U, 3), `Purge` again by the store | `PermissionDenied` |

### Workspaces

| Operation | Questions in order (whose name, concept) | Again | Refusal |
|---|---|---|---|
| A user lists or reaches a workspace (`Views::workspaces`, `workspace`) | `GetWorkspace` by the store (V, 1+4); `JoinChannel` for its classification with the user's roles (V, 2) | both again in `reached` when a file is read | left out; `WorkspaceNotFound` |
| A user works in it directly (`WorkspaceManager::reached` ← `terminal`) | `GetWorkspace` (U, 1+4); `WriteWorkspace` for a change (U, 1); `JoinChannel` (U, 2) | - | `NotFound`; `ReadOnly` and `NotCleared`, which say the workspace exists |
| In a channel (`usable_in_channel` ← `terminal_in`, `shell_in`, the tools, the prompt) | the caller takes part (no policy); `GetSession` (1+4); `channel_reading` (1+4, then 2 for a user; for an agent the copy into the same channel, answered yes); `GetWorkspace` as a filter (1+4); `Publish` of the workspace into the channel and of the channel into the workspace (2); `WriteWorkspace` for a change (1) | `may_run` asks the whole chain in every member's name for a running command | `NotUsableInChannel`, `ReadOnly` |
| Create, reclassify (`require_classification`) | `Classify` (U, 2); `Publish` into the storage's destination (U, 2); `JoinChannel` with the principal's roles (U, 2); `CreateWorkspace`/`UpdateWorkspace` by the store (1) | - | `ClassificationRefused`, `StorageNotCleared`, `NotCleared` |
| Share, rename, delete, restore, purge | the store's questions (1); `Purge` by the manager then by the store | `Purge` twice | - |

### Artifacts

| Operation | Questions in order (whose name, concept) | Again | Refusal |
|---|---|---|---|
| List, reach (`Views::artifacts`, `artifact`) | read in S, then `GetArtifact` with the artifact's attributes or, failing that, per version with the version's own list (V, 1+4); `JoinChannel` for its classification (V, 2); the content of a kept version is read without a question (issue 292) | - | left out; `ArtifactNotFound` |
| Where a version was published (`ArtifactView::publication`) | reaching that channel as the views do (`GetSession`, `channel_reading`) | - | not named |
| Publish (`ArtifactManager::publish` ← the artifact tool) | the workspace chain above for reading; `WriteArtifact` of an existing one or `CreateArtifact` (T, 1); `Publish` into the storage's destination (T, 2) | `CreateArtifact` again by the store; the existing one updated in S | `PermissionDenied`, `StorageNotCleared` |
| Keep, describe | `GetArtifact`, `UpdateArtifact` (U, 1) | `UpdateArtifact` again by the store | `NotFound`, `PermissionDenied` |
| Share, delete, restore, purge | the store's questions; `Purge` by the manager then by the store | `Purge` twice | - |

### Views, snapshots and events

| Operation | Questions (whose name, concept) | Refusal |
|---|---|---|
| A session (`Views::session`) | `GetSession` (V, 1+4); a deleted one `GetSession` again as gone | `SessionNotFound`; `Never` |
| Its channels (`SessionView::channels`, `channel`) | `channel_reading`: `GetChannel` (V, 1+4), then a user `JoinChannel` with their roles (2), an agent `CopyInformation` into its place (2+4) | `ChannelNotFound` for absent, refused and uncleared alike |
| A deleted channel (`gone_channel`) | `GetChannel` alone, no clearance | `Never` |
| A member's snapshot (`MemberView::snapshot`) | `ReadAgentContext`, `ReadAgentTools`, `ReadAgentComponents`, `ApproveToolCall`, `RetryMessage`, `CancelChannel`, `DeleteAgent` as flags (V, 1 and 3), the commands as a filter; whether approvals are shown is no question but "a user who takes part" | flags false |
| Its tools, components, context (`tool_sets`, `components`, `context`, `private_notes`) | `ReadAgentTools`, `ReadAgentComponents`, `ReadAgentContext` again (V, 1) | `Forbidden` |
| What was deleted (`deleted_*`) | the store's `ReadDeleted` per item; `Restore`, `Purge` as flags (V, 1 and 3) | - |
| An event reaching a subscriber (`EventBus`, `subscribe_as`) | per event and party: `GetAgent`/`GetUser`/`GetUserProfile`/`GetWorkspace`/`GetSkill`/`GetArtifact` by kind, every other - sessions, channels, who joined or left, an agent's state - `GetSession` with the attributes the publisher gave, a channel's for a channel's event (party's context, 1+4); in-process subscriptions ask nothing | silently not delivered |

### Users

| Operation | Questions (whose name, concept) | Refusal |
|---|---|---|
| Users, profiles (`UserManager`, the REST profile route, the stores) | the store's `Get*`/`Update*` (U, 1): a user record is the system's, a profile its user's | `NotFound`, `PermissionDenied` |
| Roles (`user_roles`, `Member::clearances`) | no question: read from the store in S wherever a clearance is decided; a member's narrowed by its personality's limiting roles, a viewer's and a manager's not | - |

## Findings

What the map shows, as facts; whether each is right is decided in issue 304. "Deliberate" means
a rule in SECURITY.md or CONFIDENTIALITY.md says so.

**Doubles**

- **F1 - the store asks again.** `UpdateSession` (attaching a workspace, creating and restoring a
  channel), `DeleteSession`, `DeleteAgent`, `UpdateChannel` (adding a participant),
  `UpdateArtifact` (keeping, describing), `CreateArtifact` (publishing), `Purge` (artifacts,
  workspaces, skills) are asked by the manager or loop before it acts and again by the store when it
  stores. That the stores check everything is deliberate (*How stores check*); that an operation
  asks the same before acting is the rule *the actor's own permission first*. Nothing names the
  pair as the design, so a reader cannot tell it from a leftover.
- **F2 - the channel admits again.** `Publish`, `ModifyMessage`, `RevokeMessage`, `Reclassify` and
  `RetryMessage` are asked by the user's actor and again as the channel admits the envelope
  (deliberate, issue 228). `AnswerQuestion` and `WakeChannel` are asked once, each in a different
  line - the actor, the loop -, a nudge never, and an approval given by a click is asked only by
  the approver, a rule's answer by the actor as well. The two lines are not drawn the same for
  every act.
- **F3 - `CallTool` twice, agent commands three times, `ApproveToolCall` up to three times** on one
  path (`ToolExecutor::admit` and `check`; filter, admission and carrying out; the actor, the
  approver's count and its strategy), each time in the same name.
- **F4 - `GetPersonality` twice**: for every agent when a session or channel is made or a
  participant added (`require_admitted`), and again when the channel is built (`Member::of`).
  Both in the agent's name; the caller is never asked whether they may read the agent or its
  personality (deliberate, P2 of issue 198).
- **F5 - the views ask, then the operation asks again**: `GetSession` by the views and by
  `open_session`; `GetWorkspace` + `JoinChannel` by `Views::workspace` and by `reached`;
  `ReadAgentTools`/`ReadAgentComponents`/`ReadAgentContext` as a snapshot's flag and by the
  operation. Deliberate: a view decides nothing (*Views*).

**Reachability (concept 4)**

- **F6 - three mechanisms, no rule.** Membership is an access-list subject (`MemberOf`), so the
  policy decides it as ownership; the views decide it by navigating, with `channel_reading`; across
  sessions it is privacy inside `CopyInformation`. `SessionLoop::channel_for` re-implements the
  user half of `channel_reading` by hand (`require_seen`); `WorkspaceManager::usable_in_channel`
  asks the agent half with the channel as its own place, so for an agent it comes down to
  `GetChannel`; `usable_in` and `shows_approvals` check "takes part" without the policy.
- **F7 - what a refusal tells differs by path.** The views answer "not found" for absent, refused
  and uncleared alike; `WorkspaceManager::reached` answers `ReadOnly` and `NotCleared` after
  `GetWorkspace`, so a workspace the viewer is not cleared for is not found in the list but exists
  on the file routes; `gone_channel` asks no clearance, so a deleted channel the user is not
  cleared for is "gone" where a live one is "not found"; `delete_channel` asks no `DeleteChannel`
  before acting and only warns when the store refuses it.
- **F8 - an agent's own session needs no `GetSession`** (`ViewsFactory::session`), a user's does
  (`Views::session`). An agent reads its session as a member; whether that is a rule or a shortcut
  is not written.

**Clearances and roles (concept 2)**

- **F9 - `JoinChannel` has two meanings.** At the door it carries the member being admitted and
  that member's roles, asked in the caller's context (`Member::require_cleared`); everywhere else
  it carries the principal and the principal's own roles (`may_see_channel`), for seeing a channel,
  a workspace, a skill, an artifact. The policy ignores the participant and decides by the roles.
- **F10 - an agent's limiting roles count at the door only.** `Member::clearances` narrows an
  agent's owner's roles by its personality; `Views::cleared_for`, `SkillManager` and
  `WorkspaceManager` take `user_roles(principal)` unnarrowed, so an agent that may not join a
  confidential channel still sees a confidential workspace, skill or artifact of its owner's. For
  channels an agent is asked `CopyInformation` instead, which asks no roles at all.
- **F11 - using a skill is decided twice differently.** A user is shown a skill by `GetSkill` and
  clearance; an agent loads one by `GetSkill`, `UseSkill` and `Publish` of the skill into its
  place, without clearance. `UseSkill` is asked nowhere else.

**The actions (concept 3)**

- **F12 - four actions the policy always allows.** `CallTool`, `ReadSecret`, `ShowCommand` and
  `ExecuteCommand` are asked of the policy and answered yes; what decides them is the place's tool
  rules and the approval strategy, the secret policy, the command's scope. They are asked so that
  another policy could refuse them; today they record a call, nothing more.
- **F13 - principal or caller.** The policy decides by the principal except `Publish`,
  `ModifyMessage`, `RevokeMessage` and `Reclassify` of a message, note or memory entry, which it
  decides by the caller (authorship); `Enforcement::require` logs every refusal with the principal.
- **F14 - `Reclassified::Note` and `Memory` carry no attributes and no kind**: a public note and a
  private note ask the same question, by author and flow alone, where a message carries the
  channel's attributes so that its owner may reclassify it too.
- **F15 - `ReadSecret` in two names**: at a call in the turn's context, with whoever asked as
  initiator; when a component is built in the agent's own, with its owner as initiator.
- **F16 - `Publish` as a flag.** `ChannelLoop::reclassify` asks `Publish` of the old classification
  into the new only to learn whether the channel was lowered.

**Where it is asked**

- **F17 - the event bus asks `GetSession` with a channel's attributes** for every event about a
  channel, its members and its agents' state, since there is no action for "may receive events
  about this channel"; in-process subscriptions are not filtered.
- **F18 - the channel's components act in the channel owner's name** (`ChannelLoop::new` builds
  them with `SecurityContext::new(owner, ...)`), the agent's in the agent's own.
- **F19 - `channel_reading` refuses unevenly**: for an agent it never answers "not at all" once
  `GetChannel` passed - it degrades to entry by entry -, for a user it does when the clearance
  fails.
- **F20 - the REST context has no place**: `AuthenticatedUser::security_context` sets no session
  and no channel, also for a request on one; nothing in the policy reads them today.
