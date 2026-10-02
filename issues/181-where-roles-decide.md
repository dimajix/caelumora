# 181 — It is not reviewed where a user's roles enter a permission check

**Status:** open
**Area:** Identity and access management

## The problem

A user has roles, given by the user provider and configured in `AppConfig`
with the clearances each grants (issue [142](142-roles.md)). An agent has
none of its own: `RuntimeManager::roles_of` gives it its owner's. With
external agents (issue [179](179-external-agents-via-a2a.md)) agents are to
have roles of their own, as people do - and then the question is not only
*what* they are, but *where they count*.

Nobody has reviewed that. As of 2026-10-02 roles reach exactly one decision,
the clearance, asked as `ActionType::JoinChannel` and decided by
`ConfidentialityRules::cleared`:

- `ChannelLoop::add_participant` (`RuntimeManager::require_cleared`),
- creating a channel and a session, templates included
  (`session.rs::require_cleared`),
- reclassifying a channel, for everyone in it (`channel.rs`),
- a user seeing a channel (`AccessQuestions::may_see_channel`, from
  `session.rs` and the views, `view/session.rs`, `view/mod.rs`).

Everything else - creating, reading, changing and deleting entities,
publishing, cancelling, approving, reading other channels - is decided by
ownership and the flow rule, without asking for a role. Whether that is
right, or an oversight, has not been asked case by case.

## Why it matters

- **Agents with roles of their own** (179) are only as good as the places
  where roles are looked at. A role that limits an external agent at the
  door, but not where it reads, changes or is handed something, limits
  less than it seems to.
- **A role that changes** takes effect at the next check - for a user at
  the next login (`accept_any` updates the roles then), for clearances at
  the next join or reclassification. A user who loses a role stays in
  every channel they joined with it, and an agent of theirs too.
- **The owner's roles for an agent** are an assumption (an agent must not
  see more than the person responsible for it), and how they combine with
  roles of the agent itself is issue [12](12-user-and-agent-permissions.md)
  in another form.

## Considerations

To be reviewed, action by action (`ActionType` in `caelumora-model/src/security.rs`):

- **Which actions should ask for a role**, and which are rightly decided by
  ownership alone. Candidates: whether anyone but the system may create or
  change personalities, templates or preconfigured components (an
  administrative role); whether reading another channel
  (`ReadChannel`) should also require the reader to be cleared for the
  source, not only the flow rule.
- **Whose roles** count when an agent acts: its owner's, its own, both
  (intersection), or the initiator's - the rule of issue 12, applied to
  roles.
- **When roles are re-checked**: only at the door, or also when they change
  - removing a participant whose clearance is gone, as reclassifying a
  channel refuses rather than removes today.
- **Consistency**: `may_see_channel` asks `JoinChannel` in the name of the
  context's initiator; `roles_of` returns nothing for an approver or the
  system. Whether every caller asks with the roles of the right subject.

## Done when

- Every `ActionType` is marked as decided by roles or not, with the reason,
  in SECURITY.md or CONFIDENTIALITY.md.
- What should ask for roles and does not, is fixed or has an issue of its
  own; the rule for whose roles count when an agent acts is decided.

## History

- **2026-10-02** — Opened at the maintainer's request while deciding where
  the roles of an external agent belong (issue 179): agents are to have
  roles as users do, and it was unclear where roles enter a permission
  check at all.
- **2026-10-02** — Moved with issue 179, step 4: the roles of a
  participant are now `Member::clearances` in `caelumora_runtime::channel`
  (a user's own, an agent's owner's, and the limit of its personality),
  asked by `Member::require_cleared` and `channel::require_cleared`; the
  roles of a user who views are `channel::user_roles`.
  `RuntimeManager::roles_of` and `require_cleared` named above are gone.
  Found on the way: the `UserController` does not follow role changes -
  they are stored at a login with no event - so roles are read from the
  store wherever they decide.
