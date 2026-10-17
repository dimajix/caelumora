# 304 — Four kinds of permission are checked in many places, and nobody has reviewed how they fit

**Status:** done
**Area:** Identity and access management
**Summary:** Four kinds of permission - ownership, flow rules, actions, reachability - are checked in many places and several layers, and nobody has reviewed where each is asked, in which order, and whether the checks that mix two of them do so on purpose.

## The problem

Who may do what is decided by four concepts that grew one after the other, each with its own
issues, its own rules and its own places in the code:

1.  **Ownership and access lists** on entities (`SecurityAttributes`: `owner_id`, `acl`; issues
    [11](11-agent-ownership.md), [23](23-acls-in-security-attributes.md)). They grant reading and
    changing of a stored thing - the `Get*`, `Update*`, `Delete*` actions - and are applied by the
    `OwnershipSecurityManager`, asked by the stores (`store::access`) and by operations that check
    themselves.
2.  **Flow rules** between classifications (`ConfidentialityRules`: the flow rule, clearances,
    compartments; issues [133](133-classification-model.md), [64](64-confidentiality-compartments.md),
    [142](142-roles.md)). They decide `Publish`, `CopyInformation`, `Classify`, `Reclassify` and the
    clearance at a channel's door (`JoinChannel`), and are applied by the same policy, as a building
    block it may use or not (issue [138](138-confidentiality-policy-in-ownership.md)).
3.  **Permissions for particular actions** - `ActionType`, 83 variants as of today. Most carry the
    attributes of an entity and are decided by concept 1; some carry none or two
    (`ApproveToolCall`, `ReadAgentTools`, `ShowAgentCommand`, `CallTool`, `ReadSecret`,
    `AnswerQuestion`, `UseSkill`) and are decided by a rule of their own, written into the
    ownership policy case by case.
4.  **Reachability through a path** - whether something can be reached at all from where one
    stands: a session through its membership (`MemberOf`), a channel of a session (`GetChannel`
    before `CopyInformation`, issue [264](264-agent-reads-channels-its-owner-may-not.md)), another
    session's channel through its privacy (`SessionPrivacy::permits`), a workspace through the
    channel it is attached to, an artifact through the channel it was published in
    (`Views::channel_of`).
    It is applied by the views (issue [163](163-navigable-views.md)), by `channel_reading`, and by
    the convention that what cannot be reached answers "not found".

Each of the four is documented - SECURITY.md for 1, 3 and 4, CONFIDENTIALITY.md for 2 - and each
has been reviewed on its own (issues [198](198-security-scenarios.md) for the actions,
[181](181-where-roles-decide.md) for the roles, [255](255-whose-name-agents-act-in.md) for whose
name). What has not been reviewed is how they fit together: where each one is asked, in which
order, and whether a check that mixes two of them does so on purpose.

What makes it hard to see today, as of 2026-10-11:

- **The same question is asked in several layers.** `Publish` has 18 call sites; `DeleteSession` is
  asked by the `SessionManager` and again by the store; `ModifyMessage`, `RevokeMessage` and
  `Reclassify` by the user's actor and again by the channel; `GetPersonality` three times while a
  session is made. Some of the repetition is deliberate - the store checks everything, an operation
  checks before it acts - but no rule says which layer owns which check, so a reader cannot tell a
  deliberate second check from a leftover.
- **Checks are spread over fifteen files and three ways of asking** (`Enforcement::require` in
  `session.rs`, `channel.rs`, `store/access.rs`, `actor/user.rs`, `artifact`, `workspace.rs`,
  the views, `skill.rs`, `command`, `tool`, `notes`, `memory`, `approval`, `secret`; the
  `AccessQuestions` with eighteen `may_*` questions; filters in the stores, the event bus and the
  command lists) - and beside them more than forty `may_*` methods on views, snapshots, approvals
  and components that wrap one of those. Which of them are entry points and which are
  conveniences is not written down.
- **One check often decides by two concepts at once.** `GetChannel` for an agent is ownership *or*
  taking part (1 and 4); `CopyInformation` is the flow rule *and* the session's privacy (2 and 4);
  `JoinChannel` is a clearance (2) asked like an action (3); `GetWorkspace` is the access list *and*
  the clearance for the workspace's classification (1 and 2). Whether that is the right place for
  each pair has not been asked.
- **Reachability is a convention, not a concept of its own.** Concept 4 has no action and no
  question: it is what the views do by navigating and what operations repeat by asking the channel
  first (`channel_reading`). An operation reached by another path - a REST call with a channel id,
  a component with a `KnowledgeRef` - has to remember to ask it.
- **The order of checks is a rule in one place only.** "The actor's own permission first" (issue
  198) says what comes first; nothing says what comes after - whether reachability, ownership,
  clearance and the flow rule are asked in a fixed order, so that a refusal leaks the same amount
  everywhere.

## Why it matters

- **A gap is found only by chance.** The security suite records what each action asks, but only
  for the actions that have a test; a new operation that reaches something by a path nobody
  thought of - or asks ownership but not clearance - passes review unless the reviewer happens to
  know all four concepts.
- **A policy outside the binary** (issue [24](24-opa-security-manager.md)) has to reproduce every
  rule that the ownership policy decides case by case. The fewer and clearer the concepts, the
  smaller that policy, and the less it can get wrong.
- **What a refusal tells** depends on the order: asked in a different order in two places, the same
  action leaks in one what it hides in the other.
- **Every later feature** - the administrator (issue [125](125-invisible-admin.md)), groups as
  subjects ([286](286-groups-as-subjects.md)), changing classified content ([169](169-changing-classified-content.md)) -
  adds to whichever concept is nearest, and the picture gets harder to review each time.

## Considerations

The review comes first, the changes after; whether the rules are to be adjusted is decided on the
map, not before.

1.  **The map.** For every place that asks the `SecurityManager` - directly, through
    `Enforcement::require`, through an `AccessQuestions` question, as a filter - record: which
    operation, in whose name, which `ActionType`, which of the four concepts decides it, and
    whether the same question is asked again on the way. The suite's recordings (issue 198) give
    the order for the actions it covers; the rest is read from the code. For every `ActionType`:
    which concept decides it, as issue 181 asks it for roles.
2.  **Which concepts are layers, which are rules.** Reachability (4) and ownership (1) say
    whether a subject may touch a thing at all; clearance and flow (2) say whether information may
    go where the action takes it; an action's own rule (3) is what remains. If they are layers, a
    fixed order follows - reach, then own, then flow, then the action's rule - and every operation
    asks it the same way, which the suite can assert. If they are not, say which pairs are rightly
    decided together and why.
3.  **Where each layer is asked once.** Candidates: the stores ask ownership and nothing else; the
    views ask reachability and answer "not found"; an operation asks its own action, and the
    channel asks the flow rule as it admits content. The second asking of one question (actor and
    channel; manager and store) is either made the rule - defence in depth, named as such - or
    removed.
4.  **Reachability as a question of its own.** Whether concept 4 should be an `AccessQuestions`
    question (`may_reach`) or an `ActionType` so that an operation reached by id asks the same
    thing the views ask by navigating, instead of each repeating `channel_reading`.
5.  **The `ActionType` family.** Whether the actions that carry no attributes, or two, belong in
    the same enum as the entity actions, or whether the enum says by its shape which concept
    decides each variant.
6.  **What not to do.** A second policy, a new trait or a new layer type is not the answer by
    itself; the codebase already has three ways to ask and that is one too many for most readers.
    Fewer entry points, each with one concept, is the direction.

## The target picture

Decided with the maintainer on 2026-10-11, on the map
([docs/PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md)). It says what is to be, not what is;
the map says what is, and the steps below lead from one to the other.

**The principle.** Every decision is made from the acting entity's `SecurityContext`, the verb,
the object's `SecurityAttributes`, the acting entity's clearances and the object's classification,
and from nothing else. Reachability decides only what is visible in a context, never whether
something is permitted: an operation that reaches an object by its id has exactly the rights of
one that navigated to it. Nothing is looked up during a decision but what the object's access list
names.

**Three decisions, orthogonal.** Each is a function of its own, and an act passes only where every
one it needs says yes.

1.  **Security** - who may do what with the object: a function of the context, the verb and the
    attributes. Access lists name three kinds of subject: `Actor` (a user; a stored list never
    names an agent directly), `Role` (from the user provider) and `Group` (below). An entry naming
    a group is resolved by one read of that group, one naming a role by the roles of the subject;
    that is the whole of what the policy learns beyond its inputs. The confidentiality verbs leave
    the `SecurityManager`.
2.  **Flow** - whether information may go where the act takes it: a function of the subject's
    clearances, the object's classification and the destination's - the channel acted in, or the
    work of the agent that reads, as it is classified now. The clearances are computed once per
    context - the principal's roles, narrowed by the caller's personality - and carried in the
    `SecurityContext`, which already carries the place. A session's privacy (issue 134) is part of
    this decision, as a fact of the source session compared with the session in the context.
    `Publish`, `CopyInformation` and the clearance at a channel's door are flow decisions;
    `Classify` is validation of the model and leaves the policy altogether; `Reclassify` splits
    into security (may the actor change this - its owner, its author) and flow (is the new
    classification admissible). This reverses issue [138](138-confidentiality-policy-in-ownership.md):
    a superuser who may act against both needs a hook in both, which is accepted.
3.  **Visibility** - what a client or an agent is shown: the views navigate and ask `Read` of each
    thing they reach, lists are filtered by `Read`, and what may not be read is not found. There is
    no rule of visibility beyond that: no `channel_reading`, no clearance asked by the views on
    their own.

**The order.** An operation asks `Read` of the object first, then its own verb for the actor, then
the flow, then anything about others - a participant's clearance, an agent's personality. A refusal
of `Read` is "not found"; every refusal after it is "forbidden" and reveals nothing about objects
the actor cannot read. The security suite asserts this order for every action it covers.

**Who asks.** Decided on 2026-10-11 as well. Every question in the map has one of five reasons;
a question with none is removed.

1.  **The entity layer always asks.** The store asks `Read` on every read and the verb on every
    write, uniformly, answering "not found" and "denied". It is the last line, and stays whether or
    not an operation asked before.
2.  **An operation asks before it stores only if it does something first** - looks at others,
    stops actors, deletes files, sends something. Then it asks its own verb first (the order), and
    the store asks again when it stores. That pair is the rule, not a double; no token tells the
    store it was asked already.
3.  **The channel decides what enters it, once, when it is submitted.** Admission moves from the
    processing of an envelope to the point where an actor hands it to the channel:
    `ChannelLoop::admit` asks in the envelope's context and answers the sender with the refusal
    instead of dropping it. Actors ask nothing for what they send - a message, a modification, a
    revocation, a reclassification, a retry, an answer, an approval by click, all alike.
4.  **Every act has one owner, and only the owner asks, once.** A tool call the `ToolExecutor` when
    it admits it, not again when it runs; a command the `CommandExecutor` when it admits it, not
    again when it is carried out; an approval the approver when it counts the answer; a secret the
    `SecretManager`; a skill the `SkillManager`. A component owns the acts on what it keeps -
    reclassifying a note, forgetting a memory entry - and asks for those itself, in the turn's
    context; it never asks for reading beyond its channel, which the views do, nor again what the
    executor asked. The `SecurityManager` stays in the `ComponentContext` for that (issue 131).
5.  **Offering is not permitting.** The views, a snapshot's flags, the filters of lists, the
    approver's `approvers_of`, the visible commands and the event bus ask to decide what to show
    or whom to wait for, never to permit; the act asks again when it is done, and that is no
    double. The form tells them apart: **enforcing asks with `Enforcement::require`, offering
    with `AccessQuestions::may_*`**, never the other way round.

**Groups** (issue [286](286-groups-as-subjects.md), the first step). A `Group` is an entity of its
own: `SecurityAttributes`, a list of actors - users and agents, no nesting - and a scope. Access
lists name it as `Subject::Group(id)`; `Subject::MemberOf`, `Membership`, the `Memberships` trait
and `StoreMemberships` go. The rules:

- Every session and every channel keeps an internal group of its participants. The controller
  that changes the participants writes the group in the same operation, before the participant,
  so that a failure between the two leaves someone reading who is about to join rather than a
  participant who may not read. It is the one writer of that group.
- An internal group lies in the scope of its session: deleted, restored and purged with it (issue
  178). It is readable through the group API, not changeable; only its session or channel changes
  it.
- A group is read by its owner and by its members; the policy reads it in the system's name when
  it resolves an entry.
- An agent may be a member of a group - the internal group of a channel holds its agents, so that
  an agent reads what it takes part in (issue 264). A stored access list still names no agent as
  an `Actor`.
- `Role` stays a subject of its own: roles come from the user provider and carry clearances;
  binding them to an entity users can change would bind the clearances with them.
- Attaching a workspace to a session shares it with `Group(session)`, which resolves issue
  [289](289-attach-compares-access.md); issue [287](287-memberships-cache.md) shrinks to the
  reading of groups. An artifact published in a channel is shared explicitly in its own list; with
  what by default - `Group(channel)`, or the actors of the moment - is decided when the artifact
  tool is touched.

**What becomes of the findings** of the map:

| Finding | Under the target picture |
|---|---|
| F1 - the store asks again | Stays: the manager asks before acting (the order), the store when it stores, as the last line. SECURITY.md names the pair. |
| F2 - the channel admits again | The channel is the enforcement, the actor's check a courtesy for an early answer. The two lines are drawn the same for every act (`AnswerQuestion`, `WakeChannel`, a nudge, an approval by click included), each decided when touched. |
| F3 - `CallTool`, commands, `ApproveToolCall` more than once | One question by the owner of the act - `ToolExecutor`, `CommandExecutor`, the approver; what is asked before is named a courtesy or removed. |
| F4 - `GetPersonality` twice, F5 - the views and then the operation | Deliberate, stay. |
| F6 - three mechanisms of reachability | Gone by the principle: `MemberOf` becomes `Group`, `channel_reading`, `require_seen`, `cleared_for` and `reached` become `Read` followed by the flow. |
| F7 - what a refusal tells | Gone by the order: "not found" for `Read`, "forbidden" after. `delete_channel` asks its own verb first. |
| F8 - an agent's own session without `Read` | The agent reads it as every viewer does, by the internal group. |
| F9 - `JoinChannel` with two meanings | Only at the door, as the flow decision about the member being admitted; seeing is `Read` and the viewer's clearance. |
| F10 - limiting roles at the door only | Gone: the clearances in the context, computed once. |
| F11 - using a skill decided twice | `Read` and `Use` (security) and the flow, for a user and an agent alike. |
| F12 - four verbs always allowed | Each gets an object with attributes - a secret, a command, a tool - or leaves the policy; decided per verb when touched. |
| F13 - principal or caller | Stays: authorship is the caller's; the refusal log names both. |
| F14 - a note's reclassification without attributes | Carries the attributes of its channel, as a message does. |
| F15 - `ReadSecret` in two names | Stays, named in SECURITY.md. |
| F16 - `Publish` as a flag | Replaced by the flow function asked as a question, not a permission. |
| F17 - events about a channel ask `GetSession` | Ask `Read` of the channel. |
| F18 - components in the channel owner's name | Decided when components are given their context. |
| F19 - `channel_reading` refuses unevenly | Gone: whole or entry by entry is the flow's answer, after `Read`. |
| F20 - the REST context without a place | Carries the place wherever a request names one. |

**The steps**, each a branch of its own, in this order:

1.  Groups (issue 286): the entity, `Subject::Group`, the internal groups of sessions and
    channels, `MemberOf` and `Memberships` removed. Stored sessions and channels, workspaces and
    artifacts with `MemberOf` entries are deleted, not converted.
2.  Clearances in the `SecurityContext`; the flow decisions out of the `SecurityManager` into a
    function of their own; `Classify` to validation; `Reclassify` split; issue 138 reversed.
3.  The order applied everywhere: `channel_reading`, `require_seen`, `cleared_for`, `reached`
    replaced by `Read` and the flow; refusals aligned; F7, F8, F11, F17.
4.  The doubles named or removed (F2, F3, F12, F16); the suite asserts the order.
5.  SECURITY.md and CONFIDENTIALITY.md rewritten to the target picture; the map redone.

## Implementation plan

Written 2026-10-11, against the code as the map describes it. Line numbers are those of the map
(`docs/PERMISSION-CHECKS.md`) and will have moved by the time a step is taken; the names will not.
Five branches, in this order; each is a step of its own that leaves the tests green, and each
records what it did under *History*. Open decisions are collected at the end.

### A. The core model (`caelumora-model`)

**A1 - `SecurityContext` carries the roles.** The flow decision needs the subject's clearances; a
clearance is a role as the configuration reads it (`ConfidentialityRules::cleared`), so what the
context carries are the *roles*: `roles: Vec<RoleId>`, the principal's, narrowed by the caller's
personality where the caller is an agent. Carrying roles rather than clearances keeps the context
free of configuration and lets the narrowing stay an intersection of roles.

- `SecurityContext::new(actor, session, channel, roles)`; `next(caller, principal, limit:
  Option<&[RoleId]>)` narrows; `system()` carries none and needs none.
- One function resolves the roles of a subject: today's `user_roles` and
  `StoreMemberships::agent_roles`, moved together to `caelumora_model::role::roles_of(store,
  actor)` (an agent: its owner's, narrowed by its personality's limiting roles). It is called where
  a context is *built*, never where one is *used*.
- Builders to change (production only): `AuthenticatedUser::security_context` (`auth.rs`), which
  reads the user's roles from the store per request - the `UserController` does not follow role
  changes (issue 181) - and gains `security_context_in(session, channel)` for every route that
  names a place (F20); `Agent::security_context` (`agent.rs`) takes the owner's roles and the
  personality, and its callers `actor/agent/mod.rs`, `actor/external_agent.rs`,
  `workspace.rs::may_run`; `User::security_context` (`user.rs`); `external_agent.rs`;
  `template.rs`; `event.rs`; `runtime.rs`; `channel.rs` where the components are built;
  `controller/user.rs`; `view/mod.rs`; `actor/user.rs`; `actor/approval.rs`; the TUI. Each gets
  its roles from `roles_of` or from a context it derives from.
- `Member::clearances`, `channel::user_roles`, `Views::roles`, `cleared_for` and every
  `user_roles(...)` call in the runtime (`channel.rs`, `session.rs`, `skill.rs`, `view/mod.rs`,
  `workspace.rs`) go: the roles are in the context. The door (`Member::require_cleared`) asks with
  the *member's* roles, resolved by `roles_of` for the member, not the caller's context.

**A2 - Access lists name groups.** `Subject::Group(GroupId)` replaces `Subject::MemberOf`; the
`Membership` enum, its `FieldValue` mapping and the REST and web UI mappings of the subject
(`caelumora-rest-server/src/models.rs`, `caelumora-web-ui/src/types`) follow.
`SecurityAttributes::validate` keeps "an `Actor` entry never names an agent" and allows any group.
`Grant` is unchanged.

**A3 - The `Group` entity** (`caelumora-model/src/group.rs`): `id: GroupId`, `name:
ScopedName`, `members: Vec<ActorId>` (users and agents, no groups), `internal: bool`, `scope:
Scope`, `security_attributes`, `deleted`. `Secured` and `Scoped`; `EntityKind::Group`; the ten
store operations every kind has (`create_group` ... `find_group_by_name`) in `store/mod.rs`,
`memory.rs`, `json.rs`, with `Actions::GROUPS` in `store/access.rs`; `ActionType::{Get, List,
Create, Update, Delete}Group`. The policy decides them as for every kind: the owner changes and
deletes. That a member reads its group is an entry of the group's own list naming the group
itself, written when it is created - resolved by the one read that resolves every group entry -,
so the policy needs no rule of its own. The existing `Scope::GroupScope { group_id: Uuid }` takes a
`GroupId`; what lies in a team's scope is a question for later, not of this plan.

**A4 - The policy resolves groups, nothing else.** A trait `Groups { async fn
members_of(&self, group: GroupId) -> Option<Vec<ActorId>> }` replaces `Memberships`;
`StoreGroups` (`store/groups.rs`, from `store/memberships.rs`) replaces `StoreMemberships` and is
bound the same way (`config.rs`, `main.rs`). `roles_of` leaves the trait: the roles are in the
context. In `OwnershipSecurityManager::granted`, `Subject::Role(r)` is `context.roles.contains(r)`,
`Subject::Group(g)` is "the members of `g` contain the principal, or the caller where reading"
(issue 264, unchanged in meaning). `NoMemberships` becomes `NoGroups`.

**A5 - The flow decisions leave the `SecurityManager`.** `ConfidentialityRules` becomes the flow
function and takes the context where it needs the subject:

- `cleared(&context, &classification)` - the context's roles clear the classification; and
  `cleared_roles(&roles, &classification)` for the door, where the roles are another's;
- `may_publish(&context, from: &Classification, into: &Classification)` - the actor is the
  caller, and `from` flows into `into`;
- `may_copy(&context, from: &Place, into: &Place)` - same channel; or same session or the
  source's privacy permits the reader's, and the entry flows;
- each with a `require_*` twin that returns the caller's error, as `Enforcement::require` does,
  logging alike.

`ActionType` loses `Publish`, `CopyInformation`, `Classify` and `JoinChannel`. `Reclassify {
target }` keeps the security half only - a channel its owner, a message the channel's owner or its
author, a note or memory entry its author, each target carrying its channel's attributes (F14) -;
the flow half is `may_classify`/`may_publish` of the new classification, asked beside it by the
owner of the act. `AccessQuestions` loses `may_publish`, `may_copy_information`,
`may_see_channel` and `channel_reading`. `OwnershipSecurityManager` no longer holds the rules
(issue 138 reversed; `allow_all` allows every *security* question and nothing of the flow, which
is a change for tests and development setups). `Classify` becomes validation:
`Classification::validate(&Compartments)` on creating or reclassifying a channel, a workspace and
a skill (`session.rs::require_classification`, `workspace.rs`, `skill.rs`), as a `Validation`
error of the model, not a refusal.

At runtime the rules are one service beside the `SecurityManager`: `RuntimeManager` holds
`Arc<ConfidentialityRules>` and hands it, at construction, to `ChannelLoop`, `ToolExecutor`,
`Views`, `WorkspaceManager`, `ArtifactManager`, `SkillManager`, `SessionManager`, the agent and
external-agent actors and the `ComponentContext` (for the acts a component owns). Nobody fetches
it through the runtime at use.

**A6 - The form.** Enforcing asks `Enforcement::require` or a `require_*` of the rules; offering
asks `may_*`. The map's offending sites are listed under C5.

### B. Runtime infrastructure

**B1 - Internal groups of sessions and channels.** `Session` and `Channel` get `group_id:
GroupId`. `SessionManager::create_session` creates the session's group, `create_channel` (and
`SessionLoop::create_channel`) the channel's, in the runtime's name, `internal: true`, in the
session's scope, owned by the session's owner; the session's and the channel's own access lists
get a `Group(own)` entry granting `Read` where they had `MemberOf(self)` (`template.rs`,
`channel.rs` in the model). The one writer of each internal group is the controller that changes
the participants - `ChannelController::add_participant`, `remove_participant`, and the session's
equivalents -, which writes the group *first* and the participant second, in the runtime's name as
scoped data is written (issue 178); a failure between the two is logged, and the next change
corrects the group. `delete_agent`'s `leave_channels` goes through the same controller methods.
Internal groups are deleted, restored and purged with their session by `delete_scoped_in` and its
siblings. Whether they are listed through the group API is an open decision (below); they are
never changed through it (`GroupManager` refuses `internal`).

**B2 - Teams.** A `GroupManager` (create, rename, add and remove members, share, delete, restore,
purge), REST routes `/groups`, and the web UI's access dialogs offering a group as a subject. The
manager checks itself as the others do (A3's actions, the store again).

**B3 - What is shared with a group.** Attaching a workspace to a session shares it with
`Group(session.group_id)` (`session.rs::set_workspace`, `create_from_template`), which closes
issue 289. Publishing an artifact in a channel writes `Group(channel.group_id)` into its list
(`artifact/mod.rs::publish`, `Artifact::read_by`) - the default is an open decision (below), the
mechanism is not. Issue 287 shrinks to the reading of groups, or closes.

### C. The sites, by rule

Every row of the map ends up with one of the five reasons or goes. What is not obvious has its
plan here; what is - a `may_*` turned into a `require`, a question dropped - is only named.

**C1 - Rule 1, the entity layer.** Unchanged. The `Group` kind joins `store/access.rs`.

**C2 - Rule 2, an operation before it stores.** Named in SECURITY.md as the pair; the sites of F1
stay. Two operations do not follow the order and are fixed: `SessionLoop::delete_channel` asks
`DeleteChannel` before it drops the channel from the session, and a refusal ends the operation
instead of being warned; `SessionManager::restore_session` asks `ReadDeleted` (through the store)
before it reads the deleted session in the runtime's name.

**C3 - Rule 3, admission at submission.** The one structural change of the runtime, with its own
plan:

1.  A new `EventSender` in `channel.rs`, `AdmittingSender { channel: Weak<ChannelLoop>, inner:
    Box<dyn EventSender> }`, wrapped around the `OriginEventSender` that `spawn_actor` gives every
    actor - so the actor still sends only through the transport `run` was handed, and the sender
    carries the sender id as issue 228 made it. Its `send` and `try_send` call `admits` and
    `admits_change` on the loop with the envelope as it is, and forward it only if both say yes.
2.  `TransportError` gains `Refused(ChannelError)`, so that the refusal reaches the actor as the
    error of its send. `process_envelope` no longer asks `admits` or `admits_change`: there is one
    way into the loop, and it admits.
3.  `admits` asks `rules.require_publish` per part (C5); `admits_change` asks `ModifyMessage`,
    `RevokeMessage`, `Reclassify`, `RetryMessage` as today, and gains the two acts the actor
    asked for alone: `AnswerQuestion` on `QuestionAnswered`/`QuestionDeclined`, and `WakeChannel`
    on `WakeupRequested` (which `ChannelLoop::wakeup` then no longer asks itself; `wakeup_actor`
    alike). A nudge stays unasked. An approval by click stays the approver's (rule 4).
4.  `UserActor` drops every `require` for what it sends: `send_message`, `modify_message`
    (`ModifyMessage` and `Publish`), `revoke_message`, `reclassify_message`, `request_retry` and
    `request_retry_of_turn` (`require_retry`), `answer_questions`; `message_to_change` keeps the
    lookup and the revoked check, which are not permissions. The actor maps
    `TransportError::Refused` to the `ChannelError` it answered before, so that the REST API and
    the web UI see what they see today.
5.  The agents keep their flow questions for what they give up or bring (`AgentActor::turn_flow`,
    `ExternalAgentActor::lowered`, `exchange`, `may_answer_in`): those are the agent's own act of
    withholding, not a pre-check of the channel's. `ToolExecutor::admitted` keeps `Publish` of a
    tool result into the agent's context: a destination of its own, before the channel's.
6.  Risk to check first: `admits_change` reads the channel's history under `state`, and the
    sender is called from an actor's task while the loop may hold `state` in `process_envelope`.
    The loop must not hold `state` across an await on the actors' output, or the admission must
    read the history through the recorder; decided when the branch starts, by reading
    `process_envelope` and `route_event`.
7.  The security suite's tests of these actions change from "the actor asks, then the channel"
    to "the channel asks once" (`a_message_is_modified_and_taken_back_by_its_author_alone`,
    `asking_an_agent_to_answer_again_is_for_whoever_takes_part`,
    `a_message_is_reclassified_by_its_author_or_the_channels_owner`, and the sending tests).

**C4 - Rule 4, one owner, once.**

- `ToolExecutor::check` drops `CallTool`; `admit` asks it once. The place's rules and the
  approval stay where they are. The test of the executor that asserts two questions asserts one.
- `AgentCommands::carry_out` drops its `admitted`; `AgentCommands::admit` asks once, in the
  issuer's context; the filter (`visible`) is offering (rule 5). `CommandExecutor::admitted` is
  then called from one place and loses the `admitted` entry point.
- `UserActor::answer_by_rule` drops `may_approve`; the approver asks `ApproveToolCall` when it
  counts the answer, for a rule's answer as for a click; `approvers_of` is offering.
- F12: `CallTool`, `ReadSecret`, `ShowCommand` and `ExecuteCommand` leave `ActionType` unless the
  decision below gives them an object with attributes. Their owners enforce what decides them
  today - the place's tool rules, the secret policy, the command's scope - and the policy is not
  asked a question it cannot answer.
- A component asks for the acts on what it keeps as today: `AgentNotes::reclassify`,
  `AgentMemory::reclassify` (security half: `Reclassify` of a note or memory entry with its
  channel's attributes; flow half: `require_publish` of the new classification). Nothing else in
  `component_impl` asks the policy; reading goes through the views.

**C5 - Rule 5, the form.** Enforcing with `may_*` becomes `require`: `ChannelLoop::admits`
(`may_publish`), `WorkspaceManager::require_classification` (`may_publish`, `may_see_channel`),
`reached` (`may_see_channel`), `usable_in` (`check` of `GetWorkspace`, `may_publish` twice),
`SkillManager::require_classification` (`may_see_channel`), `ArtifactManager::publish`
(`may_publish`), `ApproverActor::handle_response` (`require` with `|| ()` used as a flag).
`ChannelLoop::reclassify` asks no `Publish` to learn whether it lowered: it compares
`old.flows_to(new)` with the rules' compartments. Offering keeps `may_*`: the views' flags,
`approvers_of`, `visible_by`, the stores' and the event bus's filters.

**C6 - The order and the principle, site by site.**

- *One reach for channels.* `SessionView::readable` is the one function that reaches a channel:
  `Read` (security), then for a user `rules.cleared(context, channel)`, for an agent
  `rules.may_copy(context, channel as a whole, its place)` - whole, entry by entry, or not at all.
  `SessionLoop::channel_for` (`require_seen`) and `WorkspaceManager::usable_in_channel` call it
  through the views instead of asking by hand; `require_seen` goes. `Views::gone_channel` asks
  the clearance too, so that a deleted channel is "never" where a live one is "not found".
- *The agent's own session.* `ViewsFactory::session` asks `Read` of the session like
  `Views::session`; the agent passes by its channel's group.
- *Entries.* `ChannelView::may_read_entry` and `Views::may_copy_from` decide by the viewer's kind:
  an agent by `may_copy` of the entry into its place; a user reads whole channels only, so "entry
  by entry" does not arise for a user and `may_copy_from` no longer answers `true` for a viewer
  without a place.
- *Skills.* `Views::skills` (user) and `Views::usable_skills` (agent) ask the same: `Read`, `Use`
  (security), `cleared` (flow), and for the agent `may_publish` of the skill into its place.
- *Events.* `PublishedEvent::read_action` asks `GetChannel` with the channel's attributes for
  every event about a channel, its members and its agents, `GetSession` for a session's.
- *The REST context.* Routes on a session or a channel build the context with that place
  (`security_context_in`), so that the flow decisions see where the user acts.
- *Refusals.* `WorkspaceError::ReadOnly` and `NotCleared` stay as answers after `GetWorkspace`
  passed (the viewer may read the workspace); `NotCleared` on the file routes becomes
  `WorkspaceNotFound`, because the views answer that for an uncleared workspace.

**C7 - Documents and the UI.** SECURITY.md (the model, ownership, access lists with groups, in
whose name, where checks go: the five rules, the order), CONFIDENTIALITY.md (clearances from the
context, who applies the rules: issue 138 reversed, reading another channel), MEMORY.md,
TOOLS-AND-COMMANDS.md (who may approve), REST-SERVER.md and WEB-UI.md (groups, the subject
`group`), STORAGE.md (the kind), CONFIGURATION.md (`allow_all`), and the map redone from the code.
The web UI: `types/index.ts` for the subject, the access dialogs of workspaces and artifacts, a
page for teams.

**C8 - Tests.** The policy's unit tests in `security.rs` (groups, roles from the context, the
flow gone); the store suite for the group kind; the flow function's tests in
`confidentiality.rs`; the security suite: new scenarios - a team shares a workspace, the internal
group follows the participants, an observer reads a session without taking part, a role lost is
a clearance lost at the next request -, and every action whose questions change re-asserted; the
clearance tests of `session.rs` moved to the flow function where they test the rule and kept
where they test the door.

**C9 - Stored data to delete.** Everything with a `member_of` entry and everything that gets a
`group_id`: all sessions, channels, workspaces and artifacts. No reader of the old form.

### D. The branches

1.  `issue-286-groups`: A2, A3, A4, B1, B2, B3, C1, C7 and C8 as far as groups go. Behaviour
    changes only where `MemberOf` becomes `Group`; closes 286 and 289.
2.  `issue-304-flow`: A1, A5, A6, `Classify` to validation, `Reclassify` split, issue 138
    reversed, C5 for the flow sites, C7 for CONFIDENTIALITY.md.
3.  `issue-304-admission`: C3 and C4 - admission at submission, one owner once, the F12 decision.
4.  `issue-304-order`: C2 and C6 - the reach function, the refusals, the events, the REST place.
5.  `issue-304-docs`: C7 complete, the map redone; 304 closed.

### Open decisions

- **Roles or clearances in the context** (A1): roles, as proposed, or the clearances computed by
  the rules at build time. The plan assumes roles.
- **Where a request's roles come from** (A1): read from the store per request, or carried in the
  token's claims and renewed at login. The plan assumes the store.
- **`allow_all` and the flow** (A5): with the flow orthogonal, `allow_all` no longer disables it;
  tests and development that relied on that configure no compartments and no roles instead.
- **The artifact's default** (B3): `Group(channel)` or the actors of the moment.
- **Internal groups through the API** (B1): listed and readable, or not reachable at all.
- **F12 per verb** (C4): `ReadSecret` with the secret's attributes if secrets get
  `SecurityAttributes`; `CallTool` with the tool set's; or all four gone from the policy.
- **C3 step 6**: how the admission reads the history, decided after reading the loop's locking.

## Done when

- A map of every permission check - where, in whose name, which action, which concept - is in
  SECURITY.md (or a document of its own that SECURITY.md points to), with the duplicates named as
  deliberate or removed.
- Every `ActionType` is marked with the concept that decides it.
- The order in which the four concepts are asked is a rule in SECURITY.md, and the security suite
  asserts it for the actions it covers.
- What the review found to be wrong or unclear is fixed or has an issue of its own.

## History

- **2026-10-11** — Opened at the maintainer's request: it had become hard to tell at which places
  which permissions are checked, and whether the four concepts - ownership, flow rules, actions,
  reachability - are each applied clearly and in the right place.
- **2026-10-11** — The map is written, as a document of its own:
  [docs/PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md) - the four concepts and the mechanisms
  each one has, every `ActionType` with the concept that decides it, the seven kinds of code that
  ask, and every operation with its questions in order, in whose name, where it is asked again, and
  how it refuses. Twenty findings are listed there (F1-F20), as facts, for the decision: the
  doubles (store after manager, channel after actor, `CallTool` twice, agent commands three times,
  `ApproveToolCall` up to three times); reachability with three mechanisms and no rule, and
  refusals that tell different things by path; `JoinChannel` with two meanings and an agent's
  limiting roles counting at the door only; using a skill decided differently for a user and an
  agent; four actions the policy always allows; the event bus asking `GetSession` with a channel's
  attributes. No code was changed. Next: decide which concepts are layers with a fixed order
  (consideration 2), and which of the findings are deliberate, which are to be fixed.
- **2026-10-11** — The target picture decided with the maintainer and written above: every
  decision from the context, the verb, the attributes, the clearances and the classification alone;
  three orthogonal decisions - security, flow, visibility -, with the flow decisions leaving the
  `SecurityManager` (reversing issue 138) and the clearances carried in the context; the order
  `Read`, the actor's verb, the flow, then others; groups as entities (issue 286) in place of
  `MemberOf`, with internal groups kept by every session and channel. Weighed on the way and
  rejected: the participants handed to the policy as derived attributes (membership and reading
  would be one thing, an observer impossible); the join writing an `Actor` entry (explicit, but a
  second mechanism beside teams); readers inherited from a scope (an automatism, `MemberOf` by
  another name). What each finding becomes, and the five steps, are listed above.
- **2026-10-11** — Who asks, decided with the maintainer and written above as five rules: the
  entity layer always, an operation before storing only if it acts first, the channel once at
  submission with the actors asking nothing for what they send, one owner per act asking once -
  a component included, for the acts on what it keeps -, and offering apart from permitting, told
  apart by `require` against `may_*`. Applied in steps 3 and 4.
- **2026-10-11** — The implementation plan written above at the maintainer's request: the
  changes to `SecurityContext` (roles), `SecurityAttributes` (`Subject::Group`), the `Group`
  entity, the policy's `Groups` lookup, the flow function out of the `SecurityManager`; the
  internal groups and teams; every site of the map by the rule that keeps or removes it, with a
  plan of its own for admission at submission (a sender that admits, wrapped around the actor's
  transport), the reach function and the events; documents, tests, the stored data to delete;
  five branches in order; seven open decisions.
- **2026-10-11** — Step 1 done with issue [286](286-groups-as-subjects.md), branch
  `issue-286-groups`. Three deviations from the plan above, recorded there: the group of a
  session's or channel's participants has its id instead of a field `group_id`; it is written after
  the change of the participants, not before; and issue 289 is not resolved by it - attaching still
  writes an entry of its own, now naming the session's group. Issue 287 shrinks to reading groups
  and stays open.
- **2026-10-11** — Step 2 split in two, each to be reviewed on its own. **2a**, on the branch
  `issue-304-flow` (A1 of the plan): the `SecurityContext` carries `roles` and `limiting_roles`.
  The `UserController` builds a user's context, the `AgentController` - which now holds its owner's
  `UserController` - an agent's, each with the roles as they are now (issue 306); `next` carries
  none, whoever continues a context gives them (`Answering::to` takes the actor's own context).
  The agent actors and the `UserActor` build their context on every use; the REST server's
  middleware takes it from the user's controller per request. The policy matches `Subject::Role`
  by `SecurityContext::has_role`; `Memberships::roles_of`, `StoreMemberships::agent_roles` and
  `role::user_roles` are gone, and so is every lookup of roles in the runtime. `may_see_channel`
  judges by the context's roles and, for a limited caller, by the limit as well - which resolves
  F10: an agent limited by its personality is now limited where it is shown a workspace, a skill
  or an artifact, and where it writes one, not only at a channel's door. The door keeps asking
  `JoinChannel` with the member's roles, from its controller. No decision changes otherwise; the
  tests that built contexts by hand give them their roles. Stored data to delete: the state of
  agents with a turn under way and of external agents with open tasks, whose `Work` stores a
  `SecurityContext` without the new fields. **2b** - the flow out of the `SecurityManager` - is
  next.
- **2026-10-11** — **2b** done on the branch `issue-304-flow` (A5, A6 of the plan): the flow leaves
  the `SecurityManager`. `ConfidentialityPolicy` (`caelumora_model::confidentiality`) decides
  `may_publish`, `may_copy` and `cleared`, and names the configured compartments and roles;
  `ConfidentialityRules` implements it. `ConfidentialityQuestions`, by blanket implementation, gives
  the questions their names - `may_copy_information`, `cleared_for` - and the `require_*` that
  answer a refusal with the act's own error and log it alike. `ActionType` loses `Publish`,
  `CopyInformation`, `Classify` and `JoinChannel`; `Reclassify` keeps its security half;
  `AccessQuestions` loses the flow questions, `channel_reading` moves with `ChannelReader` and
  `ChannelReading` to the runtime's views, `Place` to `confidentiality`. The ownership policy holds
  no rules (issue 138 reversed), the `SecurityManager` translates no role names (the policy's
  `roles` do), `DummySecurityManager` carries nothing. The `RuntimeManager` takes the policy as a
  constructor argument beside the `SecurityManager`, `AppConfig::create_confidentiality_policy`
  builds it, and it is handed at construction to the `ChannelLoop`, the views, the `ToolExecutor`,
  the workspace, skill and artifact managers and, as a declared service, to the components of notes
  and memory. Where an act enforced a flow by asking `may_*`, it now asks `require_*`. Deviations
  from the plan: the flow is a trait, not a function of the rules, so that a policy outside the
  binary (issue 24) states it and the security suite records it - in the same list as the security
  questions, under the old names, so that the order of an action's questions shows both; that only
  who acts puts something into a channel, which `Publish` checked, is the channel's own check now
  (`ChannelLoop::admits`). Changes of behaviour: `allow_all` no longer allows every flow; a
  reclassification above the channel answers `ClassificationRefused` (400), not `Forbidden` (403),
  as a modification above it already did; classifying is validation and no longer recorded as a
  question. No stored data changes.
- **2026-10-11** — Step 3 begun on the branch `issue-304-admission`, built on `issue-304-flow`.
  Done of C4: `CallTool` is asked once, as the call is admitted, in the enforcing form
  (`require`), not again when it runs; `may_call_tool` is gone. Corrected on the way: the
  `UserActor`'s question whether its person may approve before a rule of theirs answers
  (`answer_by_rule`) stays - it is offering in the sense of rule 5, as a client asks it to show its
  buttons; the approver asks it again as it counts the answer, which is the act. Without it, a rule
  of someone who may not approve would put an answer into the channel that does not count. Found
  when C3 was to begin: the risk of step 6 does not exist - the loop routes to actors with
  `try_send` and holds its state only to store it - but checking in the actor's task opens a gap
  between the check and the entry: a channel lowered meanwhile would let in a message admitted for
  its classification before, and a revocation could overtake a modification. Admission at
  submission is to be decided by the loop, in order, with the verdict sent back to the sender;
  together with where agent commands are admitted and F12, laid before the maintainer.
- **2026-10-11** — Step 3, C3 decided by the maintainer and done: the channel decides what enters
  it, in its loop, in order, as it takes an event in (`ChannelLoop::admission`, the former `admits`
  and `admits_change` in one), first whether the sender may act so, then the flow of what it
  carries. What it refuses it answers to the sender alone with a new event,
  `ActorEvent::MessageRejected { rejected, message, reason }` with a `Rejection` (`Forbidden`,
  `ClassificationRefused`, `MessageNotFound`, `MessageUnchangeable`, `Invalid`), in the system's
  name and never in the history - after the pattern of `RetryRejected`. The `UserActor` asks
  nothing of the policy or the flow for what it sends any more (a message, a change, a
  reclassification, a request to answer again, an answer to questions - `AnswerQuestion` moved into
  the admission); it looks only at what it can see at once (the message is there, not taken back,
  an agent's). It shows what it sends before it forwards it, so that a refusal always follows what
  it refuses. `may_modify_message`, `may_revoke_message` and `may_reclassify` are gone; the REST
  routes answer such refusals no longer with `403`/`400`, but on the channel's stream. The web UI
  marks a message not let in and undoes a refused change by reading the message back; the TUI
  prints the refusal. Found on the way: modifying another's message never reaches the channel - the
  modified message speaks as its author, which only the author's actor can send (issue 228) - so
  the `UserActor` refuses it at once as `Forbidden`, structurally, without asking the policy. Not
  done: an agent is not told when the channel refuses what it said - its filter does not take
  `MessageRejected` in, and its transcript keeps the message, as before. Still open for the
  maintainer: where agent commands are admitted, F12, and whether a user's actor strikes a refused
  message from what it shows by a history entry of its own. The map (`docs/PERMISSION-CHECKS.md`)
  is redone in step 5. No stored data changes.

- **2026-10-11** — Step 3, where agent commands are admitted, decided by the maintainer (option A)
  and done: a command of an agent is the agent's act, so the agent alone asks whether the issuer
  may ask for it (`ExecuteAgentCommand`), once, as it carries it out (`carry_out`), and answers a
  refusal with `CommandFailed`, which existed already. The issuer's actor finds whom they name by
  what they see of the agents' commands (`ShowAgentCommand`, offering in the sense of rule 5) and
  asks nothing more; `Actor::admit_command` is gone, and so is the reply naming agents not asked
  of `@all`. The channel keeps to the form of the request, as before. Option B - the channel
  asking the agent's executor as it admits the request - was rejected: it would make the channel
  answer for the agent's components, and the gap between admission and carrying out would make
  the agent ask again. Checked on the way, at the maintainer's question: the list of an agent's
  commands is no event and does not wait in its queue - `Actor::commands` asks the policy through
  its executor directly, uncached, for the member snapshot, `/help` and finding whom a command
  names. Only the refusal of a command one may see but not ask for comes as late as the agent
  takes the request in; under the ownership policy both are decided by one rule.
- **2026-10-11** — Step 3, F12 decided by the maintainer per action (option C), revising the plan
  that all four leave `ActionType`. `ShowCommand` and `ExecuteCommand` take the shape of the
  commands of an agent: the command and the attributes of the channel it is run in, decided by the
  ownership policy alike - whoever reads the channel, or for a command for the owners alone the
  channel's owner. Before, they carried a name and were always allowed, so a command of the
  session marked `owners_only` was not decided by it. `CallTool` stays an action the policy
  allows, a documented hook for another policy (issue 24): the call is an object one can decide
  by, and what decides it here - the tools offered, the place's rules - is enforced where it is.
  `ReadSecret` stays as it is, with the gap it hides opened as issue
  [310](310-secrets-have-no-owner.md): a secret has no owner, so whoever writes a component that
  names one decides which credential is sent where. SECURITY.md says that until 310 only the seed
  and the configuration may write such components. No stored data changes.
- **2026-10-11** — Step 4 on the branch `issue-304-order`, built on `issue-304-admission`: C2 and
  C6, the order and the one reach.
  - C2: `SessionLoop::delete_channel` reaches the channel (`GetChannel`, else `ChannelNotFound`)
    and asks `DeleteChannel` before it takes the channel out of its session; a refusal leaves the
    channel running and listed. `SessionManager::restore_session` reads the deleted session as
    whoever restores it, no longer in the runtime's name, so that who may not see it learns
    nothing; the store asks `ReadDeleted` again as it restores (rule 2).
  - The one reach for channels: `SessionLoop::channel_for` - the REST server's way to a channel -
    reaches it through `channel_reading`, as the views do; `require_seen` is gone. A deleted channel
    is "gone" only to whom a live one would be there (`Views::gone_channel` asks
    `SessionView::readable`, the clearance included). `WorkspaceManager::usable_in_channel`
    already asked `channel_reading`.
  - The agent's own session: `ViewsFactory::session` asks `GetSession` as `Views::session` does,
    and is async with `AgentWork::session` and `Turn::session`; the agent passes by the group of
    the session's participants.
  - Entries: `Views::may_copy_from` answers no for a viewer without a place - a user reads whole
    channels only.
  - Skills: `Views::usable_skills` asks the agent's clearance too, as a user's skills are asked.
  - Events: the events of a channel, of its members and of an agent's state in it are asked as
    `GetChannel`, `AgentStateChanged` as `GetAgent`.
  - Refusals: `WorkspaceManager::reached` asks the clearance before `WriteWorkspace`, and answers a
    workspace one is not cleared for as `NotFound`, as the views do.
  - Not done, laid before the maintainer: the REST context with its place (F20). No decision reads
    the place from the context - the flow is given its places as arguments -, so it would serve the
    log alone. Left over of C5, outside this step's scope: `ApproverActor` using `require(.., ||
    ())` as a flag, `ChannelLoop::reclassify` learning whether it lowered by asking `may_publish`,
    and `WorkspaceManager::usable_in` asking `check` and `may_publish` where it enforces. No stored
    data changes.
- **2026-10-11** — Step 5 on the branch `issue-304-docs`, and done. The map
  (`docs/PERMISSION-CHECKS.md`) is redone from the code: every operation with its questions in
  order, in whose name, the concept, the reason it is asked (the five rules), what is asked again
  and the refusal; the findings split into what was decided to stay, with the reason, and what is
  open. SECURITY.md states the three decisions, the five rules and the order; CONFIDENTIALITY.md no
  longer gives the ownership policy the flow. Left over of C5, decided on the way: the approver's
  and the `ToolExecutor`'s `require` with a refusal of `()` stay - a refusal that is no error of
  theirs, logged as every act's -; `ChannelLoop::reclassify` keeps asking `may_publish` to learn
  whether it lowered, since an agent asks the same at its next step and both have to agree (the
  plan's `flows_to` would have bypassed the policy); `WorkspaceManager::usable_in` asks
  `may_read_workspace`, new beside `may_read_artifact`. Found by the map and fixed:
  `SkillManager::purge` asked `Purge` with nothing done before the store, and the `SkillManager`
  no longer takes the `SecurityManager` at all; `ArtifactManager::describe` left `UpdateArtifact`
  to the store, while `keep` asks it first since it compares the scope with the artifact's owner.
  F20 (the place in a REST context) dropped at the maintainer's agreement: no decision reads it.
  What the review found and did not fix has issues of its own: secrets without an owner
  ([310](310-secrets-have-no-owner.md)), and the order and the reach where the code does not follow
  them yet ([312](312-order-and-reach-not-everywhere.md)). Every point of *Done when* holds: the
  map, the concept per action, the order as a rule with the suite asserting it where it changed,
  and an issue for what is open. No stored data changes.
