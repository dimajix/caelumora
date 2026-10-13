# 340 — Only a person owns anything: no team can, as a company owns what is done in its name

**Status:** open
**Area:** Identity and access management

This is a memo, not yet an issue with a direction: it collects thoughts on owning things together,
so that they are not lost. Nothing in it is decided, not even whether to do it.

## The question

Every stored entity has one owner (`SecurityAttributes::owner_id`), and the owner is a person - a
user, for an agent whoever made it, for initial data the system. A team (issue
[286](286-groups-as-subjects.md)) can be granted reading or writing by an access list, but never
own anything. There is no shared ownership.

Should a team be able to own things - as a legal person, a company, owns what is done in its name
and acts in its own name through those who represent it?

## Where it came from

Issue [338](338-artifacts-go-with-their-session.md): an artifact published in a session lies in
the session's scope and goes with it; and an artifact another participant published there is not
one's own to delete. The discussion asked whether part of the problem is ownership itself - that an
artifact which is a team's result can only be one person's.

## What it might solve

What an access list cannot give, and an owner would:

- **A lifetime independent of a person.** What belongs to a team stays when the person who made it
  leaves. `docs/SECURITY.md` lists under *Not decided yet* what happens to an owner's entities when
  the owner is removed; for what is really an organisation's, this would be the answer.
- **Several who administer it.** Deleting, sharing, reclassifying and describing are the owner's
  alone. An access list can grant `Write` to a team today, so its members add versions together, but
  nobody besides the owner can manage the thing.
- **A name space of its own.** `Scope::GroupScope` exists and is used for almost nothing. A team's
  artifact would lie in the team's scope, and every member would find it there by name - for work in
  a team, the question of issue 338 how a version is added to another's artifact would rarely arise.
- **Secrets of an organisation.** Secrets have no owner yet (issue
  [310](310-secrets-have-no-owner.md)); a company's API key is the thing of a legal person.

## Teams, not sessions or channels

The group of a session's or channel's participants (issue 286) is a group as well, but makes a poor
owner:

- **Lifetime:** what a session owns goes with the session - the problem of issue 338 in another
  form.
- **Membership:** the participants change all the time and are chosen by the session's owner;
  whoever is invited later would become co-owner of everything made before. A session is a place
  where people act, not someone who acts.

## What it raises

### In whose name an action is done

A first sketch: every action is done by `(ActorId, Option<TeamId>)`. Where a team is named, the team
alone decides what may be done; otherwise the actor. The user who actually caused it is always
recorded, for the audit trail.

Open about it:

- **Who may act in a team's name**, and for what? A company acts through its representatives. If
  every member may do everything the team may, every member deletes everything of the team - less
  than today's protection. A rule of representation is needed - members change the content (what
  `Write` gives today), administrators of the team do what is the owner's (delete, share,
  reclassify) -, and a team has today only members and an owner.
- **Who chooses the hat.** Whether an action is done as oneself or as the team: the session, the
  client, the agent's configuration, a parameter of each call?
- **"The team alone decides"** means a member acting for the team reaches what the team may, also
  what they themselves may not. Is that wanted - and does it hold for confidentiality too, or is the
  acting person always checked with their own clearance?
- **The audit trail.** `SecurityContext` already tells caller, principal and initiator apart (issue
  [255](255-whose-name-agents-act-in.md)); a team would be one more "in whose name". Where the
  person is recorded - `Removal::by`, `published_by`, the logs - has to stay the person.
- **The team's own owner.** A team is owned by the user who made it; the question of who it belongs
  to moves one level up. Teams owned by teams bring cycles.

### Passive things first

Owning passive things - artifacts, workspaces, skills, later secrets - with a person always acting
as the team's representative, is much smaller than a team that acts itself. A team that owns an
**agent** becomes a principal: an agent acts with its owner's rights, so a team would need roles,
clearance, approvers for tool calls, rules for tool calls, secrets of its own. That is a second
stage, if at all.

### When something becomes the team's

`publish_artifact` would need to know whether it publishes for a person or a team: a session that
belongs to a team, a choice in the template, a parameter of the agent? Without an answer, only
things made by hand in a team's name could be the team's.

### Code that assumes a person

Much of the core takes the owner for a user: `ActorId::User(owner)` in `ArtifactManager::publish`,
`Artifact::owner() -> Option<UserId>`, `validate_users_own`, "creating only with oneself as owner",
lists of "one's own" that query `owner_id`, events delivered to the owner. Every owner check would
become a question to `Memberships` - of one group, so affordable, but a change at the centre of the
policy.

### A cheaper alternative

A grant `Manage` in the access list, given to a team: its members administer the thing besides its
owner. That gives "several who administer it" without touching ownership - but not a lifetime of its
own, nor a name space. Enough if shared administration is what is missing; not if the legal person
is.

## Done when

- It is decided whether teams own anything, and if so: who acts for a team in which matters, how an
  action names the team it is done for, and which kinds a team may own.
- Or it is rejected, with the reason recorded here.

## History

- **2026-10-13** — Opened as a memo at the maintainer's request, from the discussion of issue
  [338](338-artifacts-go-with-their-session.md). Leaning so far: teams, not sessions or channels;
  owning passive things first, which would already solve some of the problems.
