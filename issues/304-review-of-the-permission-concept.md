# 304 — Four kinds of permission are checked in many places, and nobody has reviewed how they fit

**Status:** in progress
**Area:** Identity and access management

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
