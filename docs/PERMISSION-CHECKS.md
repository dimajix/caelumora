# Permission Checks

This document is the map of every place where the kernel asks a permission question: which operation
asks, in whose name, what, which concept decides it, by which of the five reasons it is asked, and
where the same question is asked again on the way. It records what the code does at the end of issue
[304](../issues/304-review-of-the-permission-concept.md) (2026-10-11), redone from the code, and
brought up to date by issues [312](../issues/312-order-and-reach-not-everywhere.md),
[322](../issues/322-clearance-decided-outside-the-store.md) and
[593](../issues/593-every-lifecycle-change-is-guarded-in-a-way-of-its-own.md), by which an action
across objects is a procedure's. The rules it is measured against - the three decisions, the order,
the five reasons to ask - are in [SECURITY.md](SECURITY.md#where-permission-checks-go) and
[CONFIDENTIALITY.md](CONFIDENTIALITY.md); what each action is expected to ask is asserted by the
security suite (`caelumora-kernel/tests/security_suite.rs`, issue 198; an agent's actions in
`caelumora-agents/tests/security_suite.rs`, issue 370). A new action, or a change to what one asks,
changes this map as it changes the suite.

## The concepts

Who may do what is decided by four concepts. Since issue 304 each has its own function:

| No. | Concept | Decided by | Asked as |
|---|---|---|---|
| 1 | **Ownership and access lists**: who may read, use, change or delete a stored thing | the `AccessPolicy`, from `SecurityAttributes` (`owner_id`, `acl` naming actors, roles and groups) and the context's principal; the `Admission` the policy gives for a verb (issue 332) | `Get*`, `Create*`, `Update*`, `Delete*`, `Write*`, `UseSkill`, `UseSecret`, `ReadDeleted`, `HearDeleted`, `Restore`, and the acts in a conversation (`CancelChannel`, `WakeChannel`, `OpenThread`, `RetryMessage`, `AnswerQuestion`) |
| 2 | **Flow and clearance**: whether information may go where the act takes it, and who is cleared for it | the `ConfidentialityPolicy` (`ConfidentialityRules`), from the roles in the context, the classifications and the places | `may_publish`, `may_copy_information`, `cleared_for`, and their `require_*` forms; `require_classifiable` is validation |
| 3 | **Rules of particular actions**: what is neither ownership nor flow | a rule written into the policy for the variant: authorship, who answers for an agent, the system alone; or nothing, the action being a hook | `ModifyMessage`, `RevokeMessage`, `Reclassify`, `ApproveToolCall`, `ShowAgentCommand`, `ExecuteAgentCommand`, `ShowCommand`, `ExecuteCommand`, `Purge`, `LeaveChannel`, `LeaveSession`; `CallTool` |
| 4 | **Reach**: whether a thing can be reached from where one stands | reading it (1) followed by the flow (2), one question (`confidentiality::may_reach`, issue 322): the store's for what is classified - `Get*` or `ReadDeleted`, then `cleared_for` (`Stored::classification`); for a channel read from a place `view::channel_reading` over a document read in S - `GetChannel`, then for a user the clearance, for an agent the copy into its place; for an artifact the store's, the views cutting the versions by their own lists (issue 316) | no action of its own; answered "not found" |

## Which concept decides each action

From `OwnershipPolicy::check` (`caelumora-security/src/ownership.rs`). The system itself -
caller, owner and principal the system (`SecurityContext::is_system`, issue 477) - is allowed
everything before any variant is looked at. "Principal" is the context's principal - a user
themselves, an agent its owner -, "caller" the one acting - the agent itself. What is done with a
stored entity is decided by the verb, never by the kind (issue
[323](../issues/323-kinds-enumerated-in-the-core.md)); a kind's own verb by what it requires.

| Actions | Rule | Concept |
|---|---|---|
| `Get` of every kind (`GetSession`, `GetSkill`, …); a kind's own verb requiring `Read` (`UseSkill`) | system-owned, or owned by the principal or the caller, or an entry grants `Read` to the principal or the caller (`Actor`, `Group`, `Role`) - the caller for an agent reading what it takes part in (issue 264) | 1 |
| `HearDeleted` of every kind | as reading, an entry for a deleted group counting as well | 1 |
| a kind's own verb requiring `Write` (`WriteWorkspace`, `WriteArtifact`) | as reading, with `Write` | 1 |
| a kind's own verb requiring an explicit `Read` (`UseSecret`, `Requirement::Explicit`) | as reading, but by the principal alone - an agent by its owner, not as the caller -, and what the system owns admits nobody but the system: a vault the configuration grants nobody (issue [310](../issues/310-secrets-have-no-owner.md)) | 1 |
| `CancelChannel`, `WakeChannel`, `OpenThread`, `RetryMessage`, `AnswerQuestion` | the principal reads the channel | 1: reading grants acting in the conversation |
| `LeaveChannel`, `LeaveSession` | always, asked in the context of the one who leaves; that they are the caller's own is the kernel's to ask before (issues 562, 590) | 3: leaving takes nothing from anyone |
| `ReadAgentTools` | the principal reads the agent or the channel | 1, two objects |
| `ApproveToolCall` | `answers_for`: the principal reads the agent | 3, built from 1 |
| `ShowAgentCommand`, `ExecuteAgentCommand` | `owners_only`: `answers_for`; else the principal reads the channel | 3, built from 1 |
| `ShowCommand`, `ExecuteCommand` | `owners_only`: the principal owns the channel; else reads it | 3, built from 1 |
| `Create` of every kind | the new entity's owner is the principal | 1 |
| `Update`, `Delete`, `ReadDeleted`, `Restore` of every kind; a kind's own verb requiring `Owner` - `ReclassifyMemory` of the memory plugin -; `ReadAgentContext`, `ReadAgentComponents`, `StopSession` | the principal owns it | 1 |
| `Reclassify` | a channel its owner; a message the channel's owner or its author (the caller); a note its author. Whether the new classification may be given there is the flow's | 1 or 3 |
| `ModifyMessage`, `RevokeMessage` | the author is the caller | 3: authorship, not ownership |
| `Purge` of every kind | nobody but the system (issue 125) | 3 |
| `List` of every kind | always; the storage narrows the list by the admission of `Get` - of `ReadDeleted` for what was deleted -, the store filters each item it hands out with that verb | 1 per item |
| `CallTool` | always: a hook for another `AccessPolicy`; a call is decided by the tools offered and the rules of its place | 3, decided elsewhere |

Most variants are decided by the principal; `ModifyMessage`, `RevokeMessage` and `Reclassify` of a
message or note by the caller - an agent is judged as itself for what it said.
`Enforcement::require` logs a refusal with the principal either way, and hands the operation's
error what was refused (`Refused`, see [SECURITY.md](SECURITY.md)). The flow is not the
`AccessPolicy`'s at all (issue 304, reversing 138).

## The layers

| Layer | Asks | Form | Refusal |
|---|---|---|---|
| **Store** (`caelumora-store/src/access.rs`), rule 1 and reach | `Get*` on every read; `List*`, then the admission of `Get*` (`ReadDeleted` for what was deleted) handed to the storage and the verb per item as a filter; `Create*` with the new attributes; `Get*` then `Update*`/`Delete*` on the stored ones; `ReadDeleted` then `Restore`/`Purge` - always Read before the verb; after the Read, for what is classified, `cleared_for` (issue 322) | `require`, `require_cleared_for`; `may_reach` as the list filter | `NotFound` for what may not be read or is not cleared for, `PermissionDenied` for what may not be changed, `Ok(false)` for what cannot be deleted |
| **Procedures** (`caelumora-kernel/src/procedure`, a plugin's own; issue 593), rules 2 and 4 | an action across objects - the lifecycle of a session, a channel or a shared project, taking someone in or letting them go, opening or closing a thread -, once, under the locks, first in carrying it out, before anything is asked of others; ahead only what must stand before another object changes, which that object asks again as it acts (`channel::require_opening`) | `require`, `require_*`, `Participation` | the procedure's error |
| **Managers** (`SessionLoop`, `AgentManager`, `WorkspaceManager`, `ProjectController`, `ArtifactManager`, `AttachmentManager`, `SkillManager`, `GroupManager`, `SecretManager`), rules 2 and 4 | their verb before they act, where they act before they store; the flow of what they keep or make | `require`, `require_*` | the manager's error |
| **Channel loop** (`ChannelLoop`), rules 2, 3 and 4 | what changes the channel, who is admitted, and what enters it (`admission`) | `require`, `require_*` | `ChannelError::Forbidden`, `NotCleared`; `EventRejected` to the sender |
| **Owners of acts** (`ToolExecutor` - asking the kernel through the agent's runtime, issue 516 -, `CommandExecutor`, the approver, components on what they keep - asking it the same way), rule 4 | the act's own question, once | `require` | the act's own answer: a forbidden result, an answer refused (`EventRefused`, issue 567) |
| **Views** (`caelumora-kernel/src/view`), rule 5 and reach | reaching (concept 4) and the flags of a snapshot | `may_*`; `require` for reading a participant's tools, components and context | `*NotFound`, a flag false, `Forbidden` |
| **Filters** (event bus, command lists, store lists), rule 5 | the read question per item | `check` | silently left out |

The REST server asks nothing itself. It builds one context per request (`auth.rs`, without a place:
no decision reads the place from the context, the flow is given its places as arguments), reaches
a session through `procedure::session::Open` and a channel through `channel_for`, reads through the
views, and maps refusals to `403`, every `*NotFound` to `404`.

## The map

Notation for whose name: **U** the user's own context; **T** a turn's (the agent as caller, its
owner as owner and principal, whoever asked as initiator), built by the agent's runtime at every
call from the participant the channel resolved and the initiator the work names (issue 516); **A**
the agent's own (the agent itself as initiator), built so too; **E** an envelope's, built by the
channel as it admits it (issue 516): the sender's context as the channel knows it then - a user's
own, an agent's with its owner's rights and roles as the controllers hold them, limited by its
personality, the approver's and a delegate's answered for by the channel's owner -, with the
initiator the event names, and one without rights if the sender takes no part; never what the sender
set. An envelope read back from the history (issue 223) carries the recorded initiator and is
admitted again only as a command asked of an agent; **V** a viewer's (U, or T with the place the
agent reads from); **C** the owner's of a channel or session, with their roles as they are when it
is built (its components, its commands - only while they are built); **S** the system's, which asks
nothing. After the name the concept. The rule column names the reason it is asked (R1-R5,
SECURITY.md); "store" is the store layer, in the context the caller passes. Every Read of the store
of what is classified - a channel, a workspace, a skill, an artifact - is followed by `cleared_for`
in the same context (issue 322), named in the rows where it is the reason of a refusal.

The column *Again* names where a question of the operation is asked once more on the way: *(pair)*
asked by the operation itself before it acts (R2) and again by the store as it writes (R1);
*(store)* asked by the store after it was asked before - by the operation itself, by a reach or by
the store; anything else in words, with how often and by whom - in another context or by another act
too. An operation that asks once more as it writes again after `Outdated` - the store's or a
manager's - counts once: it is one write.

### Sessions

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Find (`SessionManager::find`, behind opening, stopping, deleting, moving, taking someone in and letting them go, a channel's and a thread's lifecycle) | running: `GetSession` (U,1) by hand; stored: store `GetSession` | R1 | by the views, if the client navigated first (R5) | `NotFound` |
| Make from a template (`procedure::session::Create::from_template`) | store `GetSessionTemplate`; `CreateSession` (U,1), before anything is found or made (issue 312); a chosen workspace: reached - `GetWorkspace` (U,1), `cleared_for` (U,2) -, `UpdateWorkspace` (U,1), scope; its participants resolved in its order - of a user store `GetUser`; store `GetUserProfile`; of an agent its personality found by its name: store `List(personality)`, then store `GetPersonality` of the one found; the template's workspace found or made (*Find or make* of a workspace); then *Create* without its own `CreateSession`; then each agent found or made (*Find or make an agent*) and taken into the session (*Take someone in*); then the users and agents join the channels that name them as founders (`participant::take_in`: an agent's `GetPersonality` (A,1), then `require_cleared` (U,2), nothing more - issue 420); then the workspace shared; then put into the project asked for (*Put into a project*); the session taken back in S if any of it fails | R2 + R1 | `CreateSession` (pair); `GetPersonality` once more where the agent is made (*Make an agent*); a chosen workspace's `GetWorkspace`, `cleared_for` three times (reached, reached again as it is shared, store), `UpdateWorkspace` twice (attach, store as it is shared); as *Make an agent*, *Take someone in* and *Put into a project*; an agent's `GetPersonality` again as it joins as a founder | `NotFound`, `PermissionDenied`, `PersonalityOutOfScope`, `OfAnotherPersonality`, `InAnotherStorage`, `WorkspaceOutOfScope`, `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `OutOfScope`, `ProjectOutOfScope`; undone in S |
| Create (`establish`, behind a template; `Built` in tests (`session::testing`) asks `CreateSession` first) | per channel `require_classifiable`; store `CreateSession`, per channel `CreateChannel` (U,1), the channels empty (issue 420); groups and its own project in S (issue 467); in tests, `create` then has the participants of its channels join them as founders | R2 + R1; the members: others | `CreateSession` (pair) | `ClassificationRefused`, `NotCleared`, `OutOfScope`, `PermissionDenied` |
| Open (`procedure::session::Open`) | *Find*; channels loaded in S; per agent `GetPersonality` (A,1) - refused, the participant is left out; components built in C or A (`List` + `GetComponentBundle`, `UseSecret`) | R1 | - | `NotFound` |
| Stop (`procedure::session::Stop`) | *Find*; `StopSession` (U,1) | R4 | - | `NotFound`, `PermissionDenied` |
| Delete (`procedure::session::Delete`) | *Find* - not found, `false`; `DeleteSession` (U,1); channels and its own project in S; store `GetSession` + `DeleteSession`; its own project deleted in S (issues 467, 458); the scope and its own project's listed in S, each item's store `Get…` + `Delete…` (U,1); the participants' and the members' groups deleted in S (issue 348) - something left, or these failing: the marks taken away in S; stopped | R2 + R1 | `GetSession` (store), `DeleteSession` (pair) | `PermissionDenied`, `ScopeNotEmpty` |
| Restore (`procedure::session::Restore`) | store `ReadDeleted` (U,1), as whoever restores it - its purge begun, `NotFound`; its own project found in S; a shared project it lies in read in S - deleted, the restoring refused (issue 467); `Restore` (U,1), asked ahead, since the participants' groups come back before it (issue 593); the participants' groups restored in S (issue 348); store `ReadDeleted` + `Restore`; its own project restored in S (issue 467) - failing, the groups deleted and the session marked again in S; the scope and its own project's listed in S, each item's store `ReadDeleted` + `Restore` (U,1) - refused, it stays deleted | R1 | `ReadDeleted` (store), `Restore` (pair) | `NotFound`, `PermissionDenied`, `ProjectDeleted` |
| Purge (`procedure::session::Purge`) | store `ReadDeleted` + `Purge`, the purge begun (issues 492, 496); channels - each one's state and attachments before its document -, scope and its own project with what lies in it in S; store `ReadDeleted` + `Purge` | R1 | `ReadDeleted` + `Purge` (store) | `PermissionDenied` |
| Rename, language, time zone, privacy, approval rules, description (`SessionController::update_*`) | store `GetSession` + `UpdateSession` (U,1) | R1 | - | `PermissionDenied` |
| Attach a workspace (`SessionLoop::update_workspace`) | `UpdateSession` (U,1); reached: `GetWorkspace` (U,1), `cleared_for` (U,2), else `NotFound`; `UpdateWorkspace` (U,1); scope; store `UpdateSession`; sharing it as *Share* of a workspace | R2 + R1 | `UpdateSession` (pair); `GetWorkspace`, `cleared_for` three times (reached, reached again as it is shared, store); `UpdateWorkspace` twice (attach, store as it is shared) | `PermissionDenied`, `NotFound`, `WorkspaceOutOfScope` |
| Put into a project, take out (`procedure::session::Move`, issue 467) | *Find*, as opening it, which starts it where it does not run; `UpdateSession` (U,1); the project reached: store `GetProject` (U,1), else `NotFound`; another session's own refused (structural); a new own made in S where its own was lifted; the project shared with its participants in S - the session's act was asked -; store `UpdateSession`; the share of the one it leaves taken away in S | R2 + R1 | `UpdateSession` (pair) | `PermissionDenied`, `NotFound`, `ProjectOutOfScope`, `OwnerNotAUser` |
| Take someone in (`procedure::participant::TakeIn::added`, also from `/participant add`, issue 593) | *Find*, as opening it, which starts it where it does not run; `UpdateSession` (U,1) unless in the session and a channel is named; with a channel `channel_for` (U,1-2) and `UpdateChannel` (U,1); `GetUser` or `GetAgent` (U,1); `GetPersonality` (A,1); scope; `require_cleared` with the member's roles (U,2); store `GetSession` + `UpdateSession`; group in S; with a channel store `GetChannel`, `cleared_for`, `UpdateChannel`, and the actor started - one that cannot: out of the channel and the session again in S, by the procedure's compensation (`participant::take_in`, issues 590, 593) | R2 + R1 | `GetSession` (*Find*, store); `UpdateSession` (pair); with a channel `GetChannel`, `cleared_for` (store), `UpdateChannel` twice (the procedure, the store) | `PermissionDenied`, `NotFound`, `AlreadyParticipant`, `CannotTakeIn`, `OutOfScope`, `NotCleared` |
| Join (`procedure::participant::TakeIn::joining`, also from `/channel join`, issue 593) | opening it (`procedure::session::open_to_take_in`): `JoinSession` or `GetSession` (U,1), else `NotFound`, opened in S; `GetUser` or `GetAgent` (U,1); `JoinSession` in the participant's context (M,1) unless in the session already; `JoinChannel` (M,1); `GetPersonality` (A,1); scope; `require_cleared` (U,2); store in S; with a channel stored in S and the actor started - one that cannot: out of the channel and the session again in S, the invitations given back, by the procedure's compensation (`participant::take_in`, issues 590, 593) | R2 + R1 | `JoinSession` twice (in U, then in the participant's context); stored in S | `NotFound`, `PermissionDenied`, `NotCleared` |
| Let someone go (`procedure::participant::LetGo::removed`, issues 590, 593) | *Find*, as opening it, which starts it where it does not run; `UpdateSession` (U,1); structural (`NotAParticipant`); store `GetSession` + `UpdateSession`; group in S; then each channel of it in S - a running one as *Leave*, stored in S, one that does not run in its document in S | R2 + R1 | `GetSession` (*Find*, store); `UpdateSession` (pair) | `PermissionDenied`, `NotAParticipant` |
| Leave (`procedure::participant::LetGo::leaving`, issues 590, 593) | *Find*, as opening it; structural (`NotAParticipant`); the participant the caller's own, or the caller the kernel itself (issue 598) (structural, `require_own_leave`); `LeaveSession` in the participant's context (M,1); store in S; group in S; then each channel of it in S, as *Let someone go* | R2 + R1 | - | `NotAParticipant`, `PermissionDenied` |
| Make an agent (`AgentManager::create`, behind `find_or_create`, `POST /agents` and `/agent create`) | `CreateAgent` (U,1); the holder of the scope (`ScopeHolders::require`, issue 458): store `GetSession`, `GetChannel` or `GetProject` (U,1), a channel's session in S - what lies there keeps the session from being deleted; store `GetPersonality`; scope; store `CreateAgent`; the holder again in S, afterwards - gone, the agent purged | R2 + R1 | `CreateAgent` (pair) | `Invalid`, `NotFound`, `PermissionDenied`, `PersonalityOutOfScope`, `NameTaken`, `Validation` (a project's scope, the system as owner) |
| Find or make an agent (`AgentManager::find_or_create`, behind a template) | store `List(agent)`, then store `GetAgent` (U,1) of the one of its name in exactly its scope - found if it is of the personality asked for; else *Make an agent*; if that finds its name taken, the lookup once more (issue 250) | R1, then as *Make an agent* | the lookup twice where its name was taken meanwhile | `Invalid`, `OfAnotherPersonality`; as *Make an agent* - `NameTaken` for one the caller may not read |
| Delete an agent (`AgentManager::delete`, also from `/agent delete`) | store `GetAgent`; `DeleteAgent` (U,1); store `GetAgent` + `DeleteAgent`; it rests in its channels, which follow it in S | R2 + R1 | `GetAgent` (store), `DeleteAgent` (pair) | `PermissionDenied` |
| Restore, purge an agent (`AgentManager::restore`, `purge`) | store only; restore: its mark read in S, then the store, then the holder of its scope in S (issue 458) - gone, marked again as it was | R1 | `ReadDeleted` + `Purge` (store) for a purge | `NotFound`, `PermissionDenied` |
| Rename an agent (`AgentController::update_name`) | store `GetAgent` + `UpdateAgent` (U,1) | R1 | - | `NotFound`, `PermissionDenied`, `Invalid` |

### Channels

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Reach a channel to act in it (`SessionLoop::channel_for`, the REST server's way) | `channel_reading`: `GetChannel` (U,1), `cleared_for` (U,2) | reach | by the views if the client navigated | `ChannelNotFound` for absent, refused and not cleared alike |
| Create (`procedure::channel::Create`) | *Find*, as opening it, which starts it where it does not run; `require_classifiable`; store `GetUserProfile`; members of the session (structural); the component bundles named: `List` + `GetComponentBundle` (U,1, issue 321), and what the channel would not take refused (issue 337); `CreateChannel` (U,1), before the others (issue 312); `require_admitted` of the founders (A `GetPersonality`, scope, U cleared with the member's roles); group in S, before the channel (issue 593); store `CreateChannel` (U,1), the channel empty; store `GetSession` + `UpdateSession`; components built in C (`List` + `GetComponentBundle`, `UseSecret`); then each founder joins (`participant::take_in`, issue 420): an agent's `GetPersonality` (A,1), then cleared again (U,2), stored in S - no `UpdateChannel`; one that cannot: the channel taken back in S | R2 + R1 | `CreateChannel` (pair); `List` + `GetComponentBundle` (U, then C); `GetPersonality` as they join; the founders' clearance twice (admitted, then as they join) | `InvalidComponents`, `PermissionDenied`, `ClassificationRefused`, `NotCleared`, `OutOfScope`, `NameTaken` |
| Delete (`procedure::channel::Delete`) | *Find*, as opening it, which starts it where it does not run; reached as `channel_for` reaches it: `GetChannel` (U,1), `cleared_for` (U,2) - not reached, `false`; `DeleteChannel` (U,1); store `UpdateSession` as the session drops it; store `GetChannel` + `DeleteChannel`; the scope listed in S, each item's store `Get…` + `Delete…` (U,1); its participants' group deleted in S (issue 348) - something left, or the group failing: listed and unmarked again in S; actors stopped | R2 + R1 | `GetChannel`, `cleared_for` (store), `DeleteChannel` (pair) | `PermissionDenied`, `ScopeNotEmpty`, `ChannelNotFound` (its mark no longer found, by another process) |
| Restore (`procedure::channel::Restore`) | *Find*, as opening it, which starts it where it does not run; store `ReadDeleted` (U,1), as the session lists its deleted channels; `Restore` (U,1), before the others (issue 312); `require_admitted` as creating; its participants' group restored in S (issue 348), before it (issue 593); store `ReadDeleted` + `Restore`; whoever is no longer in the session let go in S (issue 590), their clearance not asked; store `GetSession` + `UpdateSession` - failing, unlisted and marked again in S, with its group; the scope as for a session | R2 + R1 | `ReadDeleted`, `cleared_for` (store), `Restore` (pair); `GetPersonality` again as it is built | `ChannelNotFound`, `PermissionDenied`, `NotCleared`, `NameTaken` |
| Rename, language, description, workspace switch, approval rules (`ChannelController::update_*`, also from `/channel rename`, `summary`, `briefing`) | store `GetChannel` + `UpdateChannel` (U,1) | R1 | - | `PermissionDenied` |
| Reclassify (`procedure::channel::Reclassify`) | *Find*, as opening it, which starts it where it does not run; reached as `channel_for` reaches it: `GetChannel` (U,1), `cleared_for` (U,2), else `ChannelNotFound`; `Reclassify` of the channel (U,1); `require_classifiable`; per participant `require_cleared` (U,2); store `UpdateChannel`; `may_publish` of the old into the new to learn whether it was lowered | R2 + R1; the last a question | - | `NotFound`, `ChannelNotFound`, `Forbidden`, `ClassificationRefused`, `NotCleared` |
| Remove a participant (`LetGo::removed` with the channel, also from `/participant remove`) | *Find* the session and reach the channel (`channel_for`); structural (`NotAParticipant`); store `GetChannel` + `UpdateChannel` (U,1) | R1 | `GetChannel`, `cleared_for` (`channel_for`, store) | `PermissionDenied` |
| Leave (`LetGo::leaving` with the channel, also from `/channel leave`, issue 562) | *Find* the session and reach the channel (`channel_for`); structural (`NotAParticipant`); the participant the caller's own, or the caller the kernel itself (issue 598) (structural, `require_own_leave`); `LeaveChannel` in the participant's context (M,1); store in S - whether it could run is not asked | R2 + R1 | - | `Forbidden` |
| Take someone into a thread (`procedure::participant::TakeIntoThread::added`, issue 602) | *Find*, as opening it, which starts it where it does not run; reach the channel (`channel_for`); `UpdateChannel` (U,1), before anything of the participant; structural (`NotAParticipant` for one the channel does not list or that cannot be resolved); then the channel: the thread open, its circle not closed, the actor running, not there already, one that takes part in several threads (structural) - the document unchanged, nothing stored but the join in the history | R2 + R1 | `GetChannel`, `cleared_for` (`channel_for`) | `PermissionDenied`, `NotAParticipant`, `ThreadNotOpen`, `ClosedCircle`, `AlreadyInThread`, `InOneThreadAlone` |
| Join a thread (`TakeIntoThread::joining`, issue 602) | as *Take someone into a thread*, but instead of `UpdateChannel`: the participant the caller's own (structural, `require_own_act`); `GetChannel` in the participant's context (M,1) | R2 + R1 | `GetChannel` (`channel_for`, and in the participant's context) | as *Take someone into a thread* |
| Let someone go from a thread (`LetGoFromThread::removed`, issue 602) | *Find*, as opening it; reach the channel (`channel_for`); `UpdateChannel` (U,1); structural (`NotAParticipant`); then the channel: the actor running there, not its last thread (structural); its turn there cancelled - the document unchanged, nothing stored but the leave in the history | R2 + R1 | `GetChannel`, `cleared_for` (`channel_for`) | `PermissionDenied`, `NotAParticipant`, `NotInThread`, `LastThread` |
| Leave a thread (`LetGoFromThread::leaving`, issue 602) | as *Let someone go from a thread*, but instead of `UpdateChannel`: the participant the caller's own (structural, `require_own_act`); `LeaveChannel` in the participant's context (M,1) | R2 + R1 | - | `Forbidden`, as *Let someone go from a thread* |
| Cancel the channel or a thread of it, one actor, one tool call (`cancel`, `cancel_actor`, `interrupt_tool_call`) | `CancelChannel` (U,1); a thread named open (structural) | R4 | - | `Forbidden`, `ThreadNotOpen` |
| Wake the channel or a thread of it, one actor (`wakeup`, `wakeup_actor`, from `/agent wakeup`) | `WakeChannel` (U,1); a thread named open (structural) | R4 | `/agent wakeup` asked its own command questions first | `Forbidden`, `ThreadNotOpen` |
| Open a thread (`procedure::thread::Open`, issues 557, 593) | *Find*, as opening it, which starts it where it does not run; reached as `channel_for` reaches it: `GetChannel` (U,1), `cleared_for` (U,2); `OpenThread` (U,1), before the others; a person at the line alone, the origin's thread open (structural); each agent it is to own taking part in the session not yet (structural, issue 598); each founder who takes part in the channel already the caller's own (structural, `Participation::require_own`, issue 602); then each founder new to the channel taken into the session as a founder, before the thread is there (`participant::take_into_session`): read (`GetUser`, `GetAgent`, U,1), the caller's own (structural, `Participation::require_own`), `GetPersonality` (A,1), scope, `require_cleared` (U,2), stored in S unless in it - no `UpdateSession`; then the thread opened, `OpenThread`, the person at the line and the origin's thread asked again by the channel as it opens it (`ChannelLoop::open_thread`), and each newcomer listed in the channel, stored in S - no `UpdateChannel` -, its actor started, each who takes part already taken into the thread (*Take someone into a thread*, without `UpdateChannel`); last, where its circle is open, the author of the message it hangs at drawn in, asking nothing (`ChannelLoop::draw_in_author`, issue 602); one that cannot: those taken in out of the channel and the session again and the thread closed in the opener's name (`ChannelLoop::close_thread`: the opener, refused while anyone takes part), by the procedure's compensation | R2 + R1 | `OpenThread` twice (the procedure ahead, the channel as it opens it) | `ChannelNotFound`, `Forbidden`, `NotAtTheLine`, `ThreadNotOpen`, `AlreadyParticipant`, `PermissionDenied`, `NotCleared`, `OutOfScope`, `InOneThreadAlone` |
| Delegate into a thread (`ChannelLoop::delegate`, issue 557) | in the giver's - the caller's - name: `GetChannel` (U,1); the thread open and opened at another, its circle closed (issue 602), the recipient an agent running in it, the giver where it was opened, a delegation of the thread before by the same giver to the same recipient, not let go (issue 598), the thread not quiet (structural); recorded by `run`, waited for, the brief given by the thread's delegate and admitted as `GivenToRead` | R1 | the admission of what the delegate gives | `Forbidden`, `ThreadNotOpen`, `OpenCircle`, `NotInThread`, `ThreadQuiet`, `DelegatedOtherwise`, `DelegationLetGo` |
| Close a thread (`procedure::thread::Close`, issue 593) | *Find*, as opening it; reached as `channel_for` reaches it: `GetChannel` (U,1), `cleared_for` (U,2); whoever opened it: nothing; anyone else `UpdateChannel` (U,1); nobody taking part in it (structural) - whoever lets someone go from it, or opens it, holds it meanwhile; a thread that owns agents (issue 598): these asked ahead with the agents it owns aside (`ChannelLoop::require_closing`), then each owned agent let go from the session by its owner's own act, or the kernel's, in whose context (`SecurityContext::system`) its follower closes a thread whose delegation was let go (`LeaveSession` in the agent's context, *Leave a session*; one let go already passed over); then each owned agent deleted (`DeleteAgent`, U,1); closed and recorded by `run`, which asks again whether anyone takes part, as a thread may have drawn someone in meanwhile (issue 602) | R1 | whether it may be closed, and whether anyone takes part, once more as it is closed | `Forbidden`, `ThreadNotEmpty`, `PermissionDenied` |

### What enters a channel

`ChannelLoop::admission`, in the envelope's context (**E**), in the loop, in order, against the
channel as it is when the event is taken in; a rejection is answered to the sender alone with
`EventRejected`. The actors ask nothing for what they send. All rows R3; the sender's own
permission first, then the flow of what it carries.

| Event | Questions in order | `Rejection` |
|---|---|---|
| any event | its sender is E's caller by construction; a sender that takes no part has no rights | - |
| a message, a turn's messages | per part - the message, each attachment and each tool result - `require_publish` (E,2) | `ClassificationRefused` |
| a message, a turn's messages | what a result gave (`files`, issue 512) and `shown` (issue 282) lies in the channel's key space (`AttachmentManager::lies_in`, structural) and may flow into the channel as every part | `Invalid`, `ClassificationRefused` |
| a message said (`CompleteMessageGenerated`, `IntermediateMessageGenerated`) | in a thread whose circle is closed, from one taking part there (structural, issue 602); whoever says it, if of no turn, and whom it mentions are drawn into a thread whose circle is open as it enters (`ChannelLoop::draw_in`), asking nothing of their own: its sender may write in the channel, and whom it draws in takes part in the channel, which is all a thread asks | `ClosedCircle` |
| a message said (`CompleteMessageGenerated`, `IntermediateMessageGenerated`), last | its attachments uploaded by the caller for the channel, as uploaded and not claimed, at most as many as configured, none twice - checked and claimed at once (`AttachmentManager::claim`, structural) | `Invalid` |
| `MessageModified` | the message is there; `ModifyMessage` (E,3); it can be changed so; the flow per part; last, its attachments those it carried or as for a message said | `MessageNotFound`, `Forbidden`, `MessageUnchangeable`, `ClassificationRefused`, `Invalid` |
| `MessageRevoked` | there; `RevokeMessage` (E,3); changeable | as above |
| `MessageReclassified` | there; `Reclassify` of the message (E,1 or 3); `require_publish` of the new classification (E,2); changeable | as above |
| `RetryRequested` | `RetryMessage` (E,1); a message, not taken back, or a turn that can be answered again (`retryable_turn`, issue 567), of an agent taking part in the thread it is made in, the message's or the turn's (issue 602) | `MessageNotFound`, `Forbidden`, `MessageUnchangeable`, `Invalid` |
| `QuestionAnswered`, `QuestionDeclined` | `AnswerQuestion` (E,1); whether its sender was asked is the agent's | `Forbidden` |
| `CommandRequested` | from a user (structural); the agent named takes part in the thread it is sent in (issue 602) and carries the command out (structural); `ShowAgentCommand`, `ExecuteAgentCommand` (E,3) with the agent's attributes and the channel's - asked again, with the issuer's rights as they are then, when the request is read back from the history for the agent (issue 516) | `Invalid`, `Forbidden` |
| `NudgeRequested` | form only: from a user, naming someone taking part in the thread it is sent in (issue 602) | `Invalid` |
| `GivenToRead` (issue 557) | from the delegate of a thread opened, as the thread's delegation says: into it the open brief to its recipient, back where the thread was opened what came of the open brief, of the recipient to the giver; in the delegating chain's name; to whom runs in that thread; a text, without attachments, tool calls or results (structural); the flow of the message as for a message said (E,2) | `Invalid`, `ClassificationRefused` |
| `ApprovalAnswered` | from a user, saying their own part alone (`decision`, issue 419, structural); which agent asked and `ApproveToolCall` are the approver's (issue 567) | `Invalid` |
| `ApprovalGranted`, `ApprovalDenied`, `ApprovalTimedOut` | from an approver alone (structural) | `Invalid` |
| `EventRefused` | to someone else taking part (structural, issue 567) | `Invalid` |
| everything else | nothing | - |

Before sending, the `UserActor` refuses only what it cannot send: a message not there to modify or
to ask to answer again - it reads it to make the modification of, or to address the request -,
another's message to modify - a modification speaks as its author, which only the author's actor
can send (issue 228) -, an attachment that is none of the user's uploads for the channel. Whether a
message can be changed, taken back, reclassified or answered again the channel decides alone (issue
567).

### Tool calls, approvals, commands, secrets

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Admit a tool call (`ToolExecutor::admit`) | offered; asked of the kernel (`ActorRuntime::admit_tool_call`, issue 516): `CallTool` (T,3), the place's rules; approval | R4 | not again as it runs: offered, an agent, the place's rules (`ActorRuntime::rule_denying`) | a forbidden result |
| A tool's result (`ToolExecutor::admitted`) | `may_publish` of the result into the turn (T,2), asked of the kernel (`ActorRuntime::may_publish`) | R4: the agent's context is a destination of its own | the channel as the result enters it (R3) | the result withheld |
| Tool sets offered (`ToolExecutor::tool_sets`) | per set with a destination `may_publish` of the channel into it (T,2), asked of the kernel (`ActorRuntime::may_publish`) | R5 | - | the set not offered |
| Who may approve (`ApproverActor::approvers_of`) | per user `may_approve` (their context as the channel knows it, `ChannelLoop::context_of`, 3) | R5 | - | left out; nobody, denied at once |
| An approval by click (`ApproverActor::handle_answer`) | admitted by the channel by its form (*What enters a channel*); the request held, its agent's attributes read in S, then `ApproveToolCall` of the sender in the context the channel admits them in (`ChannelLoop::context_of`, 3, issue 567) | R4 | `approvers_of` for the strategy (R5) | `EventRefused` to its sender: `Invalid` for a request not held or an agent not read, `Forbidden` |
| An approval by a person's rule (`UserActor::answer_by_rule`) | `may_approve` (U,3), then as a click | R5 | three times per answer: whether the rule answers, whether the approver counts it, whom to wait for | the rule does not answer |
| A command of the session (`CommandExecutor::execute`) | `ShowCommand`, `ExecuteCommand` with the channel (U,3) | R4 | `/agent wakeup` asks `WakeChannel` itself, `/participant add` the lookup of `SessionView::candidates` by the whole name (the session and the channel read, `UpdateSession` asked to decide whether to tell why one cannot be taken in, and that for each found if so), then what taking in asks; `/participant list` reads the channel through its view (`GetSession`, `GetChannel`, `cleared_for`); `/participant remove` reads the channel as `/participant list` does, then asks what removing asks; `/agent list` reads the session (`GetSession`) and lists its agents (`List`, `GetAgent` each); `/agent delete` reads them so, then asks what deleting asks; `/agent usage` reads the channel as `/participant list` does, and each agent's state through its view without asking more; `/agent create` the lookup of `Views::personality_named`, then what making an agent asks; `/channel list` reads the session (`GetSession`) and each of its channels (`GetChannel`, `cleared_for`), `/channel describe` the session and the channel named so; `/channel notes` reads the channel so, and its agents' notes through its view without asking more; `/channel rename`, `summary` and `briefing` ask what changing the channel asks; `/channel create` what creating a channel asks; `/channel delete` reads the channel named as `/channel describe` does, then asks what deleting asks; `/channel join` finds the running channel by its name without reading it, then asks what joining asks; `/channel leave` asks what leaving asks, and with an agent named first reads the channel as `/participant list` does; `/workspace run` asks what the workspace in a channel asks for a change (*In a channel* of the workspaces, `WriteWorkspace`), then its call what admitting a message of the issuer asks, its result's flow (`require_publish`) included (issue 587) | `Unknown`, `Forbidden` |
| Commands shown (`CommandExecutor::visible`, `/help`, completion) | per command `ShowCommand` (U,3) | R5 | - | left out |
| A command of an agent asked for (`UserActor::request_command`) | whom it names: the agents' commands as the issuer sees them (`ChannelLoop::commands_of`: `ShowAgentCommand`, U,3) | R5 | at the door (*What enters a channel*) | `Unknown`, `InvalidArguments` |
| An agent carries a command out (`AgentBody::carry_out`) | nothing: admitted at the door (issue 516) | - | - | `CommandFailed` for one it does not carry out |
| A secret (`SecretManager::resolve`) | `UseSecret` of its vault (1), and of each fallback asked - T at a call, of the secrets the kernel binds to the work (`ActorRuntime::secrets`, issue 516), C as a component of the kernel is built, S for a storage | R4 | - | `SecretError::PermissionDenied` |
| An extension handed out (`ActorRuntime::reader`, `writer`: a skill's, an artifact's, the workspace's, where the stores keep the work's session and channel) | `may_read_session` (T,1), as the work's session is reached; what the face asks it asks as it is used | reach | at each use | none handed out (`None`) |

### What an agent reads beyond its channel

An agent's views are built per piece of work, with the agent's place, and handed to it as readers
(`SessionReader`, issue 516); all in **T**.

| Operation | Questions in order | Rule | Refusal |
|---|---|---|---|
| Its own session (`ActorRuntime::session`) | `may_read_session` (T,1) - by the group of the session's participants | reach | `SessionNotFound` |
| A channel by name or id (`SessionReader::channel`, `channel_named`; `read_history`, `read_notes`) | `channel_reading`: `GetChannel` (T,1), `copyable_information` of the channel into the place (T,2) - as a whole or entry by entry; entry by entry what it answers (`Reading`) judges each message or note by its classification, a page of the history as a filter in the store (issue 421) | reach | `ChannelNotFound`, `NoChannelNamed` alike |
| All channels (`channels`, `all_channels`) | as above per channel | reach | left out |
| Where the stores keep its session and channel (`StorePlacesReader`, the kernel's extension, issue 577; the prompt part of `store_places`) | `channel_reading` of its own channel, as `SessionReader::channel` asks it, as the reader is bound | reach | none |
| Its own notes of its channel (`NotesReader::own`, the notes plugin, issue 607; the prompt parts of `notes` and `private_notes`, the numbers of new notes) | its session reached as the agent reaches it, `GetSession` (T,1); store `GetNotes` of its notes' document in its channel (T,1), as the reader is made | R4: the agent's own | none read |
| The notes of a channel (`NotesReader::published_in`; the prompt part of `notes`, `read_notes`) | `channel_reading`, as `SessionReader::channel` asks it (T,1-2); the documents of the agents taking part read in S, each note as `Reading` admits its classification; nothing released of a channel read entry by entry: `NoChannelNamed` | reach | `ChannelNotFound`, `NoChannelNamed` alike |
| Its own private notes of other channels (`NotesReader::own_private_elsewhere`, the notes plugin, issue 607; the prompt part of `private_notes`, `/private-notes`) | its session reached as the agent reaches it, `GetSession` (T,1); no `GetChannel` - its own knowledge -: the channels it takes or took part in read in S (`SessionView::channels_taken_part_in`), its notes there in S; per note `may_copy_from` (T,2) | reach | left out |
| Its memory (`MemoryReader::recalled`, the memory plugin, issue 518; the prompt part of `agent_memory`, `/memory`) | its session reached as the agent reaches it, `GetSession` (T,1); store `GetMemory` of the memory's document (T,1); no `GetChannel` - its own knowledge -; `may_copy_information` of the entries' origin into the work's place (T,2), once per origin and classification, by the origin's privacy then and - for another session, looked up in S - now, decided by the plugin as it makes the reader | R4: the agent's own knowledge, as its private notes | left out |
| Commit what a turn changed of its memory (`ActorRuntime::commit_staged`, `MemoryWriter`, issue 518) | the work's session reached as the agent reaches it, `GetSession` (T,1); under the agent's lock, store `GetMemory` (T,1); per change: an entry changed recalled at the work's place (T,2) or added before in the same turn, a new one of a number reserved and never given; `may_publish` of its classification into the work's (T,2); a reclassification as below; all or none; store `CreateMemory` or `UpdateMemory` (T,1) | R4 | `CommitError::Unreachable`, `Invalid`, `Refused`, `Store`, logged |
| Commit what a turn changed of its notes (`ActorRuntime::commit_staged`, `NotesWriter`, issue 607) | the work's session reached as the agent reaches it, `GetSession` (T,1); under the lock of the notes of the agent in the channel, store `GetNotes` (T,1); per change, of the agent's chains in the work's channel alone: a new note of a number reserved (`NotesWriter::reserve_id`) and never given there, nor added before in the same turn, a note changed one that stands or was added before; `may_publish` of its classification into the work's (T,2); a reclassification as below; all or none; store `CreateNotes` or `UpdateNotes` (T,1); announced to the readers of the channel, running or not (`NOTES_CHANGED`) | R4 | `CommitError::Unreachable`, `Invalid`, `Refused`, `Store`, logged |
| Reclassify a note (`TurnNotes::reclassify`, the notes plugin, issue 607) | the note is there; asked of the plugin's writer (`NotesWriter::may_reclassify`): `Reclassify` of the note, by its author, with its channel's attributes (T,3), `require_publish` of the new classification within the work (T,2) - asked again as the turn's change is committed | R4 | `InvalidParameter`, `PermissionDenied` |
| Reclassify a memory entry (`TurnMemory::reclassify`) | the entry recalled; asked of the plugin (`MemoryWriter::may_reclassify`), and again as it is committed: `ReclassifyMemory` of the memory's document, its owner's (T,1), `may_publish` within the work's classification (T,2) | R4 | as above |

### Skills

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| A user lists or reads skills (`Views::skills`, `skill`) | store `GetSkill` (V,1), `cleared_for` (V,2) | R1 + reach (store) | - | left out; `SkillNotFound` |
| An agent's catalogue, `list_skills`, `load_skill`, `/skills`, `/skill`, compaction (`SkillReader::usable_skills`, `usable_skill_named`) | store `GetSkill` of what is seen from the place (T,1), `cleared_for` (T,2); `UseSkill` (T,1); `may_publish` into the place (T,2) | reach (store) | - | left out; `NoSkillNamed` |
| Create (`SkillManager::create`) | `require_classifiable`; `require_cleared_for` (U or T,2); in a session's scope the holder of the scope (`ScopeHolders::require`, issue 458): store `GetSession` (U or T,1); store `CreateSkill` (1); the holder again in S, afterwards - gone, the skill purged | R4 (flow) + R1 | - | `ClassificationRefused`, `NotCleared`, `PermissionDenied`, `NotFound`, `NameTaken` |
| Change, reclassify (`SkillManager::update`) | store `GetSkill`, `cleared_for` (2); for a classification `require_classifiable`, `require_cleared_for`; store `GetSkill`, `cleared_for` + `UpdateSkill` | reach (store) + R1 (+ flow) | `GetSkill`, `cleared_for` (store) | `NotFound`, `ClassificationRefused`, `NotCleared`, `PermissionDenied` |
| Delete, restore, purge (`SkillManager`) | store only: `GetSkill` or `ReadDeleted`, `cleared_for`, then the verb; a deletion reads first; restore: its mark read in S, then the store, then the holder of its scope in S (issue 458) - gone, marked again as it was | R1 + reach (store) | `GetSkill`, `cleared_for` (store) for a deletion | `PermissionDenied`, `NotFound` |

### Workspaces

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| A user lists or reaches one (`Views::workspaces`, `workspace`) | store `GetWorkspace` (V,1), `cleared_for` (V,2) | R1 + reach (store) | by `reached` for a file route | left out; `WorkspaceNotFound` |
| A user works in it directly (`WorkspaceController::terminal`, the file routes) | store `GetWorkspace` (U,1), `cleared_for` (U,2), else `NotFound`; for a change `WriteWorkspace` (U,1) | reach (store), then R4 | - | `NotFound`, `ReadOnly` |
| In a channel (`WorkspaceController::usable_in_channel`, behind `terminal_in` and `shell_in`: the workspace's extension for the terminal, artifact and attachment tools and the workspace prompt - the reader's terminal, which reads alone, the writer's terminal and shell (issue 516) -, the REST server's to end a command, `/workspace run` to run one in a person's name, issue 587) | session and channel as the running session holds them, as stored for the `CommandReaper`; `may_read_session` (1); `channel_reading`; `usable_in`: the channel uses it, the caller takes part, scope, `may_read_workspace` (1), `may_publish` both ways (2); for a change `WriteWorkspace` (1) | reach, then R4 | - | `NotUsableInChannel`, `ReadOnly` |
| A running command goes on (`may_run`, as the channel's participants or the workspace change) | the chain of *In a channel* per current participant | an act that lasts, asked again as what permitted it changes | per participant and event | the command ended |
| The channel's workspace offered (`WorkspaceReader`, the kernel's extension) | `usable_in` (V) | R5 | - | none |
| Create (`WorkspaceManager::create`, behind `find_or_create` and `POST /workspaces`) | `require_classifiable`; `require_storable` into the storage's destination (U,2); `require_cleared_for` (U,2); in a session's scope `CreateWorkspace` (U,1); the holder of the scope (`ScopeHolders::require`, issue 458): store `GetSession`, `GetChannel` or `GetProject` (U,1), a channel's session in S - what lies there keeps the session from being deleted; store `CreateWorkspace` (U,1); the holder again in S, afterwards - gone, the workspace purged with its place | R4 (flow) + R1 | `CreateWorkspace` (pair, in a session's scope) | `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `PermissionDenied`, `NotFound`, `NameTaken` |
| Find or make (`WorkspaceManager::find_or_create`, behind a template and the terminal program) | store `List(workspace)`, then store `GetWorkspace` (U,1) and `cleared_for` (U,2) of the one of its name in exactly its scope - found if it lies in the storage asked for; else *Create*; if that finds its name taken, the lookup once more (issue 250) | R1 + reach (store), then as *Create* | the lookup twice where its name was taken meanwhile | `Invalid`, `InAnotherStorage`; as *Create* - `NameTaken` for one the caller may not read or is not cleared for |
| Change, reclassify (`WorkspaceController::update_*`) | reached (store `GetWorkspace` U,1, `cleared_for` U,2); a classification as creating; store `GetWorkspace`, `cleared_for` + `UpdateWorkspace` | reach (store) + R1 (+ flow) | `GetWorkspace`, `cleared_for` (store) | `NotFound`, `ClassificationRefused`, `StorageNotCleared`, `NotCleared`, `PermissionDenied` |
| Share (`update_access`, `share`) | reached; `require_nameable`: store `GetGroup` per newly named group - the caller's own question (issue 312); store `GetWorkspace`, `cleared_for` + `UpdateWorkspace` | reach (store) + R1 | `GetWorkspace`, `cleared_for` (store) | `NotFound`, `PermissionDenied`, `Validation` |
| Delete, restore (`WorkspaceManager`) | delete: reached, else `false`; restore: its mark read in S, the store - `ReadDeleted`, `cleared_for`, then the verb -, then the holder of its scope in S (issue 458) - gone, marked again as it was | R1 + reach (store) | `GetWorkspace`, `cleared_for` (store) for a deletion | `PermissionDenied`, `NotFound` |
| Purge (`purge`) | store `ReadDeleted` + `Purge`, the purge begun (issue 492); files removed; store `ReadDeleted` + `Purge` | R1 | `ReadDeleted`, `cleared_for` + `Purge` (store) | `PermissionDenied` |

### Projects

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| List, reach (`Views::projects`, `project`) | the shared ones asked for alone (`Filter::shared_projects`); store `GetProject` (V,1) - its owner, its members by their group, whom its list names, the participants of a session in it | R1 + reach (store) | - | left out; `ProjectNotFound` |
| Its sessions (`ProjectView::sessions`) | store `GetSession` (V,1) per session that lies in it (`Filter::in_project`) | R1 | - | left out |
| Make (`ProjectManager::create`) | the scope (structural); store `CreateProject` (U,1); its members' group in S - failing, the project marked deleted and purged in S | R1 | - | `OwnerNotAUser`, `InvalidScope`, `PermissionDenied`, `NameTaken`, `Validation` |
| Find or make (`ProjectManager::find_or_create`, behind the terminal program) | the scope (structural); store `List(project)`, then store `GetProject` (U,1) of the one of its name in exactly its scope; else *Make*; if that finds its name taken, the lookup once more (issue 250) | R1, then as *Make* | the lookup twice where its name was taken meanwhile | `InvalidScope`, `Invalid`; as *Make* - `NameTaken` for one the caller may not read |
| Change (`ProjectController::update_name`, `update_description`; `procedure::project::Lift`, by `ProjectController::lift`) | reached: store `GetProject` (U,1), else `NotFound`; for lifting a session's own alone, into the global scope or its owner's (structural, under the lock of its writing), read in S first as it is locked, to lock its session too; store `GetProject` + `UpdateProject` | reach (store) + R1 | `GetProject` (store) | `NotFound`, `InvalidScope`, `PermissionDenied`, `Validation`, `NameTaken` |
| Set members (`update_members`) | reached; `UpdateProject` (U,1), before whether the users exist is asked in S (issue 312); store `GetProject` + `UpdateProject`; its members' group in S | reach + R2 + R1 | `GetProject` (store), `UpdateProject` (pair) | `NotFound`, `PermissionDenied`, `UnknownMember`, `Validation` for a session's own |
| Share (`update_access`, `share`, `unshare`) | reached; `require_nameable`; store `GetProject` + `UpdateProject` - in S where a session shares the project it lies in | reach (store) + R1 | `GetProject` (store) | `NotFound`, `PermissionDenied`, `Validation` |
| Delete (`procedure::project::Delete`) | store `GetProject`; a session's own refused (structural); store `GetProject` + `DeleteProject` (U,1); its sessions listed in S, each deleted as *Delete* of a session in the deleter's name; its scope listed in S, each item's store `Get…` + `Delete…` (U,1); its members' group in S - something left: the mark taken away in S | R1 | `GetProject` (store); as deleting a session, per session | `InvalidScope`, `PermissionDenied`, `ScopeNotEmpty` |
| Restore (`procedure::project::Restore`) | store `ReadDeleted` (U,1); its purge begun, `NotFound`; a session's own refused (structural); `Restore` (U,1), asked ahead, since its members' group comes back before it (issue 593); its members' group in S; store `ReadDeleted` + `Restore` - failing, its members' group deleted again in S; its sessions deleted with it restored as *Restore* of a session, its scope as a session's - refused, they stay deleted | R1 | `ReadDeleted` (store), `Restore` (pair); as restoring a session, per session | `NotFound`, `InvalidScope`, `PermissionDenied` |
| Purge (`procedure::project::Purge`) | store `ReadDeleted`; a session's own refused (structural); store `ReadDeleted` + `Purge`, the purge begun (issues 492, 496); its sessions, its scope and its members' group in S; store `ReadDeleted` + `Purge` | R1 | `ReadDeleted` three times, `Purge` twice (store) | `InvalidScope`, `PermissionDenied` |

### Artifacts

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| List, reach (`ArtifactViews::artifacts`, `artifact`, `ArtifactChannelViews::artifacts`, `ArtifactProjectViews::artifacts` of the plugin `caelumora-artifacts`) | listing: the teams the viewer belongs to, found as `Views::groups` finds them (V,1), named besides them and their roles (`Views::subjects`); store `GetArtifact` (V,1), `cleared_for` (V,2); the versions cut: per version `may_read_artifact` (`ArtifactAccess`) with the version's list (V,1) | R1 + reach (store) | - | left out; `ArtifactError::NotFound` |
| Where a version was published (`ArtifactView::publication`) | `Views::channel_of`: the session, then `channel_reading`; gone, as a live one would be reached | reach | - | not named |
| Publish (`ArtifactManager::publish`, the artifact tool) | the workspace's reach (*In a channel*); the project of the name reached (`ScopeHolders::require`, issue 458): store `GetProject` (T,1); an existing artifact found in S: `WriteArtifact` (T,1) - one not the caller's to add to is refused as such, not taken for a name to make anew; or `CreateArtifact` (T,1); `require_storable` into the storage's destination (T,2); content kept; store `CreateArtifact` for a new one; the project again in S, afterwards - gone, the new artifact purged with its content | R2 + R1 | `CreateArtifact` (pair) | `NotFound` (the project), `InvalidScope`, `PermissionDenied`, `ClassifiedOtherwise`, `StorageNotCleared` |
| Share (`set_access`) | store `GetArtifact` (U,1), `cleared_for` (U,2); `require_nameable`; store `GetArtifact`, `cleared_for` + `UpdateArtifact` | reach (store) + R1 | `GetArtifact`, `cleared_for` (store) | `NotFound`, `PermissionDenied`, `Validation` |
| Describe (`describe`) | store `GetArtifact`, `cleared_for`; store `GetArtifact`, `cleared_for` + `UpdateArtifact` | reach (store) + R1 | `GetArtifact`, `cleared_for` (store) | `NotFound`, `PermissionDenied`, `Invalid` |
| Delete, restore (`ArtifactManager`) | the store: `GetArtifact` or `ReadDeleted`, `cleared_for`, then the verb; a deletion reads first to announce it; a restoring reads its mark in S first and asks the project in S afterwards (issue 458) - gone, marked again as it was | reach (store) + R1 | `GetArtifact`, `cleared_for` (store) for a deletion | `PermissionDenied`, `NotFound` |
| Purge (`purge`) | store `ReadDeleted` + `Purge`, the purge begun (issue 492); content removed; store `ReadDeleted` + `Purge` | R1 | `ReadDeleted`, `cleared_for` + `Purge` (store) | `PermissionDenied` |

### Sub-agents

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Start (`SubAgentWriter::start`, the plugin `caelumora-subagents`, issue 558) | the work's channel reached as a frontend reaches it (`SessionLoop::channel_for`): `GetChannel` (T,1), `cleared_for` (T,2); the work no deeper a sub-agent than `plugins.subagents` allows (`max_depth`, structural, issue 597), and fewer sub-agents open in its tree than allowed (`max_open`, structural); the personality found from the channel's place as the kernel finds one by name (`Views::personality_named`: `GetPersonality`, V,1), an internal one (structural), limited at least as the work's context is (`SecurityContext::covers_limit`, structural, issue 597); the agent made (`AgentManager::create`): `CreateAgent` (T,1) for the work's owner, the channel's scope (`GetChannel`, T,1), `GetPersonality` (T,1); the thread opened with it as its founder (*Open a thread*: `OpenThread`, the founder the owner's own); no more sub-agents open in the work's tree, its own counted, than allowed (`max_open`, structural); the brief delegated (*Delegate into a thread*); a step that fails rolls back what was done, in the work's name | R2 + R1 | as each building block asks | `UnknownPersonality`, `External`, `ClearedWider`, `TooDeep`, `TooMany`, `Forbidden` - what the access policy refused, without the rule -, `Failed` - the channel not reached answered as not found, as `channel_for` answers it |
| Start by command (`/subagent`, `SubagentCommand`, issue 596) | asked for at the channel's door (`CommandRequested`): `ExecuteAgentCommand` for the agent's owners alone (`answers_for`, E,3); the personality one the component lists (structural); then *Start* in the command turn's context (T: the owner's rights, the issuer as initiator), at the message the command gives the agent; one whose text cannot be given ended again (*End*, in the same context) | R3, then as *Start* | as *Start* | `Unknown` to whoever may not issue it; `InvalidArguments` - a personality not listed, no brief, what *Start* answers that the issuer can correct -, `PermissionDenied` - `Forbidden`, what the access policy refused, without the rule, and `TooDeep` (issue 596) -, `Failed` |
| Ask (`ask`) | the channel reached as above; the thread's delegation given by the work's agent (structural); delegated again (*Delegate into a thread*) | R1 | - | `NoSubAgent`, `Forbidden`, `Failed` |
| End (`end`) | the channel reached as above; the thread's delegation given by the work's agent (structural); its thread closed with the sub-agent it owns, in the work's name (*Close a thread*, issue 598: by its opener, the sub-agent let go by its owner's own act, `DeleteAgent`, T,1) | R1 | the channel reached again by the closing | `NoSubAgent`, `Forbidden`, `Failed` |

### Attachments

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| Upload (`AttachmentManager::create`) | the channel as it stands, handed over by whoever reached it in the caller's name (the REST server: `SessionLoop::channel_for`, reach); the caller listed among its participants (structural); `require_classifiable`; `require_publish` into the channel (U,2); `require_storable` into the storage's destination (U,2); content kept | reach + R2 | the channel's admission of the message that claims it | `NotAParticipant`, `ClassificationRefused`, `StorageNotCleared` |
| Download (`ChannelView::attachment_content`) | the message read as the viewer sees it in the history (V, reach); the attachment one it carries - or one a tool result of it gave or showed its model, which the viewer is shown only with the result (`Message::carried`, `Message::published`) -, which a channel read entry by entry releases by its own label (`ChannelView::attachment`, `may_read_entry`); content read asking nothing (`AttachmentManager::content_of`) | reach | - | `MessageNotFound` (`NoChannelNamed` entry by entry), `AttachmentError::NotFound` |
| An agent's tools (`attachments`) | the number resolved in the agent's state (structural); the attachment read through the view of its channel as its owner (V, reach), as a download - a message the history does not hold yet found only as what the kernel kept for this caller in this channel (`AttachmentManager::kept_content`, structural); `copy_attachment` then what writing into the workspace asks (`WorkspaceWriter::terminal`), `attach` a copy waiting for the answer as an upload does, of the attachment named by its message and id - not handed over - and read by the agent's `ActorRuntime` through the same view (`copy_attachment`, issue 549; `AttachmentManager::copy`, given the channel as the runtime holds it: the agent listed) | reach (+ the workspace's) | the channel's admission of the answer that claims the copy | `InvalidParameter` for a number not given, `NotFound` |
| What a tool gave its model (`ActorRuntime::create_content`, `AttachmentManager::create_content`, issue 512) | by the `ToolExecutor`, for a result not withheld, given the channel as the agent's `ActorRuntime` holds it; the agent listed among its participants (structural); as many as a message carries attachments (structural); `require_classifiable`; `require_publish` into the channel (U,2); `require_storable` into the storage's destination (U,2); all kept or none | R2 | the channel's admission of the message of the result (`lies_in`) | `NotAParticipant`, `TooMany`, `ClassificationRefused`, `StorageNotCleared`: the call fails |
| Whether a tool can keep what it gives (`Turn::can_keep_content`, `ActorRuntime::can_keep_content`, `AttachmentManager::can_keep_content`, issue 553) | by a tool, before it hands over what nobody configured it to keep (the original of a page `fetch_page` fetched), given the channel as the agent's `ActorRuntime` holds it; what `create_content` checks before it keeps anything, of the files the tool would hand over: the agent listed (structural); their number, names and sizes (structural); `require_classifiable`; `require_publish` into the channel (U,2); `require_storable` into the storage's destination (U,2); nothing kept | R2 | all of them by `create_content`, as what was handed over is kept | `false`: the tool hands it not over |
| Given a model along (`ActorRuntime::load_attachments`, `AttachmentManager::load`) | that each lies in the key space of the agent's channel, whatever its message says (structural, issue 549): what lies there it heard, or cannot name, since its key holds a random id; the turn sees each attachment's label as it is given (`Work::see`) | - | - | `Altered`, `NotFound`, `Content`: the call fails |
| Purge with a channel's state (`purge`, behind `procedure::session::Purge`) | in S, after the session's purge was permitted | - | - | - |

### Groups

| Operation | Questions in order | Rule | Again | Refusal |
|---|---|---|---|---|
| List, reach (`Views::groups`, `group`) | the teams the viewer owns or belongs to asked for alone (`Filter::teams`, issue 317), store `GetGroup` (V,1) | R1 | - | left out; `GroupNotFound` |
| Make a team (`GroupManager::create`) | store `CreateGroup` (U,1) | R1 | - | `PermissionDenied`, `NameTaken` |
| Set members (`set_members`) | store `GetGroup`; a group that is no team refused (structural); `UpdateGroup` (U,1), before whether the users exist is asked (issue 312); the users read in S; store `GetGroup` + `UpdateGroup` | R2 + R1 | `GetGroup` (store), `UpdateGroup` (pair) | `NotFound`, `NotATeam`, `PermissionDenied`, `UnknownMember` |
| Rename (`GroupManager::rename`) | store `GetGroup`; a group that is no team refused (structural); store `GetGroup` + `UpdateGroup` | R1 | `GetGroup` (store) | `NotFound`, `NotATeam`, `PermissionDenied` |
| Delete, restore, purge (`GroupManager::delete`, `restore`, `purge`) | store `GetGroup` or `ReadDeleted`; a group that is no team refused (structural); store `GetGroup` + `DeleteGroup`, `ReadDeleted` + `Restore` or `ReadDeleted` + `Purge` | R1 | `GetGroup` (store) for a deletion, `ReadDeleted` (store) otherwise | `NotFound`, `NotATeam`, `PermissionDenied` |
| Name a group in an access list (`require_nameable`) | store `GetGroup` (U,1) per newly named group | R1 | - | `Validation` |
| Drop entries for groups that are gone (`prune_gone_groups`, issue 347) | per group the lists name, read in S, also among the deleted, before a controller's document or an artifact is stored | - | - | - |
| The participants' and members' groups (`GroupManager::keep_participants`, `delete_participants`, `restore_participants`, `purge_participants`) | in S, after the change of the participants or members, or the deletion, restoring or purge of their session, channel or project, was permitted | - | - | - |

### Views, snapshots and events

| Operation | Questions | Rule | Refusal |
|---|---|---|---|
| Sessions (`Views::sessions`, `session`) | store `GetSession`; running, `may_read_session` (V,1) | R1 + reach | left out; `SessionNotFound` |
| A deleted session or channel (`gone_session`, `gone_channel`) | read in S; `may_read_session`; a channel as a live one is reached (`SessionView::readable`, the clearance included) | reach | `Never` |
| Channels (`SessionView::readable`) | `channel_reading` (V) | reach | `ChannelNotFound` |
| Agents, personalities (`Views::agents`, `agent`, `personalities`, `personality_named`) | store `GetAgent`, `GetPersonality` (V,1); an agent's personality named in S; by name, only those seen from the place, the innermost | R1 | left out; `AgentNotFound`, `NoPersonalityNamed` |
| An agent's channels (`AgentView::channels`) | the sessions that have it, store `GetSession` (V,1); their channels as `SessionView::channels` reads them | R1 + reach | left out |
| Flags of a session, channel, member, agent, workspace, skill, group, artifact | `may_change_session`, `may_change_channel`, `may_read_agent_context`, `may_read_agent_tools`, `may_read_agent_components`, `may_approve`, `may_retry_message`, `may_cancel_channel`, `may_change_agent`, `may_delete_agent`, the member's commands, `may_change_workspace`, `may_write_workspace`, `may_change_skill` (`SkillAccess`), `may_change_group`, `may_change_artifact`, `may_write_artifact` (`ArtifactAccess`) | R5 | a flag false |
| A participant's tools, components, context, private notes (`tool_sets`, `components`, `context`; the notes plugin's `SessionAgentNotes::private_notes_of` through `require_read_context`, its documents then read in S, each note as `Reading` admits it) | `ReadAgentTools`, `ReadAgentComponents`, `ReadAgentContext` (V,1), the context also only of a channel read whole | R4: reading them is the views' act | `Forbidden` |
| What was deleted (every `deleted_*`, through `Views::deleted_of`) | store `ReadDeleted` per item; flags `may_restore` - false for one whose purge is begun (issue 492) -, `may_purge`; for a session's channels also `may_change_session`, once, without which none may be restored - `may_purge` is asked, and none is purged alone | R1 + R5 | - |
| An event reaching a subscriber (`EventBus::subscribe` in someone's name; the kernel's own, as the system, are not asked) | per event, of every such subscriber at once, `may_reach_each` - `may_reach` as the store asks it, by `check_each`, which reads a group once per question - for an event about a part, once for the whole and once for the part (issue 503): the read question - `GetChannel` for a channel's events, its members' and an agent's state in it; `GetAgent`, `GetUser`, `GetUserProfile`, `GetWorkspace`, `GetSkill`, `GetGroup`, `GetArtifact` by kind; `GetSession` for a session's; `HearDeleted` instead for a deletion -, and for what is classified `cleared_for` the classification it has, or had as it is reclassified (issues 312, 322) | R5 + reach | not delivered |

### Users

| Operation | Questions | Rule | Refusal |
|---|---|---|---|
| A profile read or changed, approval rules (`UserController`) | store `GetUserProfile`, `UpdateUserProfile` (U,1); the first change `CreateUserProfile` (U,1) | R1 | `NotFound`, `PermissionDenied` |
| A channel marked read (`UserController::mark_read`) | store `GetChannel` (U,1), the clearance included; the message from where the user's view begins - of a thread other than the line, said in it (issue 595) -, structural; the state is the user's own, stored without a question | R1 | `NotFound` for the channel and the message alike |
| Users, agents, personalities, profiles read by the kernel (controllers, names in the views) | in S | - | - |
| Templates listed | store `GetSessionTemplate` per item (U,1) | R1 | left out |
| Roles, limiting roles | no question: carried by the context, put there by the `UserController` and the `AgentController` from the roles as they are now, the limit from the `PersonalityController` as it last took the personality in | - | - |

## Findings

### Decided to stay

- **Pairs (rule 2).** An operation that does something before it stores asks its verb, and the
  store asks again - every row whose *Again* names a pair: creating, deleting a session, a channel
  or an agent, attaching a workspace, setting members, among them. No token tells the store it was
  asked already.
- **The agents' own flow.** An agent asks the flow of what it gives up or brings in its own name -
  its turn still flows into the channel (`AgentActor::lowered`), an external agent's remote side,
  what a request brings, a lowered channel (`ExternalAgentActor`). That is the agent's act of
  withholding, not a pre-check of the channel's, which asks again as anything enters it (rule 3).
  Likewise a tool's result is asked into the agent's context before the channel asks it into
  itself: two destinations.
- *Memory.** What an agent may recall of its memory, its plugin decides by the flow of each entry's
  origin - its own knowledge, reached without asking for the channel it came from, as its private
  notes of other channels are. Notes and memory are named in their plugins, each with its own
  enforcement; a generic one for every component's state was rejected (issue
  [65](../issues/65-generic-component-state.md)).
- **The reach is one function for offering and acting.** `channel_reading` and
  `WorkspaceController::usable_in` are built from `may_*` and answer with an option: the views turn
  `None` into "not there", an operation into "not found". It is neither enforcing nor offering, but
  the one way to reach a thing, so that both see the same.
- **A refusal that is no error.** The `ToolExecutor` asks `require` with a refusal of `()` and
  turns it into its own outcome, a forbidden result, so that the refusal is logged as every act's
  is.
- **Filters with `check`.** The event bus, the command lists and the store's list filter ask the raw
  question, since they take any action.
- **Lowered.** `ChannelLoop::reclassify` asks `may_publish` of the old classification into the
  new one to learn whether the channel was lowered - the question an agent asks at its next step,
  so that both agree.
- **Running commands.** A command in a workspace lasts; whether it may go on is asked again, in
  every participant's name, whenever the participants or the workspace change (`may_run`).
- **A new version of an artifact.** `publish` asks `WriteArtifact` of an artifact found in the
  kernel's name, so that one not the caller's to add to is refused as such rather than taken for a
  name to make anew.
- **`GetPersonality` twice.** An agent's personality is read as the participant is admitted and
  again as the channel is built: two acts.
- **The place in a REST context.** No decision reads it; the flow is given its places as arguments.
- **`CallTool`** stays a hook for another `AccessPolicy`. A secret is decided by its vault's
  access list (`UseSecret`, issue [310](../issues/310-secrets-have-no-owner.md)).

### Decided in issue 312

- **Validation and the actor's own flow before the verb.** What tells the actor only about their
  own request - its form, the structure of the reached object, the flow of what they ask for,
  whether they may read a group they name - may come before their permission, which the store
  then asks alone: the workspace, skill and artifact managers, `require_nameable`. The
  group manager's `NotATeam` and `remove_participant`'s `NotAParticipant` are structure, seen
  by whoever reached the object.
- **Three `ApproveToolCall` for an answer by a rule.** Whether the person's rule answers at all
  (offering, so that no refusal is sent in their name), whether the approver counts the answer,
  and whom it waits for: three reasons, one each.
- **`UpdateWorkspace` twice for attaching and sharing.** Attaching asks before the session is
  changed, sharing is stored by the store's question alone.
- **A channel's components in its owner's name** are issue
  [321](../issues/321-channel-components-in-the-owners-name.md), decided below.
- **What is listed is the store's to decide.** A view does not filter a store's list again: the
  lists of what was deleted show what `ReadDeleted` lets through, and restoring and purging reach it
  alike.

### Decided in issue 322

- **The store reaches what is classified, the clearance included.** Channels, workspaces, skills
  and artifacts answer their classification (`Stored::classification`), and the store asks
  `cleared_for` after every Read, the deleted alike. The views and the managers no longer ask it of
  what they read through the store; an operation asks it twice where it reads before it stores,
  as it asks `Get*` twice.
- **What the views read in the kernel's name stays theirs.** They read channels and artifacts in S
  and decide the reach themselves: a channel's depends on where the reader reads from - an agent
  reads one it is not cleared for as a whole entry by entry -, which a context does not carry; an
  artifact's on its versions' lists until issue
  [316](../issues/316-store-checks-artifact-without-versions.md).
- **One question of reach.** The store's list filter asks `confidentiality::may_reach`, the event
  bus `may_reach_each`, which answers each subscriber as `may_reach` does, so that both reach alike.

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

- **Reading a version needs the artifact's list and the version's**, as in HDFS and ADLS. The store
  checks an artifact by its own list, as every kind, and the views read artifacts in V, no longer in
  S; they cut the versions by their own lists (`Views::readable_artifact`). Publishing lets the
  channel's participants read the artifact as well as the version, in the kernel's name for whom
  `WriteArtifact` lets add a version. An event about a version reaches whoever reads both
  (`EventBus::publish_classified_part`). No method merges the lists for the store: a predicate over
  an entity's own attributes (issue [332](../issues/332-policy-as-a-predicate.md)) holds for
  artifacts as for every kind.

### Open

None from the map.
