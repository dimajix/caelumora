# 317 — The viewer's teams are listed by reading every group

**Status:** done
**Area:** Identity and access management

## The problem

`Views::groups` lists the teams a viewer owns or belongs to by asking the store for every group in
the viewer's name: the store reads each and asks the policy `GetGroup` of it, and the view drops the
participants of sessions and channels afterwards. That is the scan issue
[293](293-store-queries.md) moved the lists away from.

Issue [308](308-artifacts-shared-with-a-team-not-listed.md) added `GroupField` and with it a query
for the groups an actor is a member of, which `Views::artifacts` uses to name the viewer's teams.
The teams of a viewer are now found in two ways: by a query for `Views::artifacts`, by a scan for
`Views::groups`.

## Why it matters

- Every list of groups reads every group and asks the policy once per group, however many there are.
- "The teams of the viewer" has two answers made in two ways, which can drift apart.

## Considerations

- **A query for `Views::groups`.** `GroupField` gains the owner, and `Views::groups` asks for the
  groups the viewer owns or is a member of - in the runtime's name, as `Views::artifacts` asks, or
  in the viewer's, narrowing the scan to these. One method of the views then gives the viewer's
  teams to both lists; `Views::artifacts` keeps only those they are a member of, since the policy
  resolves a group by its members.
- **Leave it.** Groups are few compared to sessions and channels; the scan costs little today.

## Done when

- `Views::groups` asks the store for the viewer's groups by a query instead of reading every group.
- The viewer's teams are found in one place for both lists.

## History

- **2026-10-11** — Opened at the maintainer's request, found during issue
  [308](308-artifacts-shared-with-a-team-not-listed.md).
- **2026-10-13** — Done, by a query. `GroupField` gained the owner, and `Group::teams` is the query
  for what lies where a team lies - the global scope or a user's -, which needed a value of the
  query for a variant whatever it holds (`FieldValue::variant`, `?` in Postgres): the scope of a
  team names its owner, so no value of its own could name every team. `Views::teams`, private,
  asks the store in the viewer's name for the teams they own or belong to; `Views::groups` lists
  them, and `Views::artifacts` names the viewer by those they belong to - no longer found in the
  runtime's name. The trash of groups asks for teams alone as well. The security suite records
  that listing a viewer's teams asks of those alone.

