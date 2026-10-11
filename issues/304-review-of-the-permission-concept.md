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
