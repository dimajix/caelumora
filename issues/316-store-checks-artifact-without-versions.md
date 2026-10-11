# 316 — The store checks an artifact without the lists of its versions

**Status:** open
**Area:** Identity and access management

## The problem

Since issue [23](23-acls-in-security-attributes.md) an artifact is read by whom its own access list
grants it, or a version by whom that version's list grants it. The store knows only the first half:
its check of a read (`Access::readable`, `store/access.rs`) asks `GetArtifact` with
`Secured::security_attributes()`, the artifact's own list. An artifact that reaches the viewer only
by a version's list - every artifact published in a channel reaches its participants this way - is
refused by the store in the viewer's name.

So `Views::artifacts` and `Views::artifact` ask the store in the runtime's name and decide
themselves, in `Views::readable_artifact`: `may_read_artifact` with the artifact's list, else per
version with the version's. What "may read an artifact" means is split between the store and the
views, and the views go around the check every other kind is read through.

## Why it matters

- The views ask the store as the system, which AGENTS.md names as a sign of a responsibility on the
  wrong object: the store's check is the one that should answer.
- A second reader of artifacts - another frontend, a tool - that asks the store in its caller's
  name gets a different answer than the views, and has to repeat `readable_artifact` to get the
  right one.

## Considerations

- **The store checks the versions too.** An artifact is readable among a list, or by id, when its
  own list or a version's grants it. `Secured` hands out one set of attributes; an artifact would
  need several - its own, and per version `Artifact::version_attributes`. Either `Secured` grows a
  way to name further sets that any of may grant reading, or the store's read of artifacts gets a
  check of its own. The views then ask in the viewer's name and only cut the versions the viewer
  may not read.
- **The store cuts the versions as well.** It would hand out only the readable versions. That is
  the views' work - what of an entity a viewer is shown -, not the store's.
- **Leave it.** The views are the one reader today; the split stays documented in
  `Views::artifacts`.

The first is the likely answer. It changes what the security suite records as asked when artifacts
are listed or read, since the store then asks the policy in the viewer's name.

## Done when

- `Views::artifacts` and `Views::artifact` ask the store in the viewer's context.
- What may read an artifact is decided in one place.

## History

- **2026-10-11** — Opened at the maintainer's request, found during issue
  [308](308-artifacts-shared-with-a-team-not-listed.md): asked why `Views::artifacts` lists in the
  runtime's name, the reason was that the store's check knows the artifact's list and not those of
  its versions.
