# 316 — The store checks an artifact without the lists of its versions

**Status:** done
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
- **2026-10-11** — Issue [322](322-clearance-decided-outside-the-store.md) moved the clearance of
  workspaces and skills into the store (`Stored::readers_cleared_for`). An artifact's follows here,
  once the store checks its versions' lists: then `Artifact` answers its classification, and the
  views' and `ArtifactManager::reached`'s own `cleared_for` fall away.
- **2026-10-11** — Issue 322 then moved an artifact's clearance into the store as well
  (`Stored::classification`): it does not depend on the versions' lists. What is left here is
  reading by those lists; the views still read artifacts in the runtime's name and ask the
  clearance themselves until then.
- **2026-10-13** — Done, but by none of the options above: the model of the lists changed, decided
  with the maintainer. A first implementation of the first option - `Stored::reader_attributes`,
  the artifact's list with all its versions', asked by the store for `Get` - was taken back: it
  would have put a rule of one kind into the store's check, which a database could only evaluate
  by being taught it per kind (issue [332](332-policy-as-a-predicate.md)). Instead, as in HDFS and
  ADLS, **reading a version needs the artifact's list and the version's**: the artifact's grants
  reading it - its name, description and which versions it has - and adding to it, a version's
  grants reading that version. The store checks an artifact by its own list, as every kind; the
  views (`Views::artifacts`, `artifact`, `ChannelView::artifacts`) read in the viewer's name and
  cut the versions by their own lists (`readable_artifact`); the views' `cleared_for` went, since
  the store asks it. What changed besides:
  - Publishing lets the participants of the channel read the version and the artifact
    (`Artifact::grant_reading`, `ArtifactVersion::grant_reading`) - in the runtime's name where
    whoever adds a version does not own the artifact.
  - Sharing (`set_access`) hands whom the list newly names to every version there is, unless asked
    not to (`?versions=false`); whom it no longer names reads no version. A version published later
    reaches its channel alone; a default list for new versions is issue
    [339](339-default-access-of-new-versions.md).
  - An event about a version reaches whoever reads the artifact and the version
    (`EventBus::publish_classified_part`); those about the artifact go by its own list.
    `Artifact::reader_attributes` went.
  - `Views::artifacts` finds an artifact by its own list alone; an artifact read but none of whose
    versions is readable is shown without versions, while a channel lists only those with a
    version published there that the viewer reads.
  - Kept: the lists of the versions, editable later, since an artifact is not bound to a channel
    or a session; that it goes with its session is a bug of its own, issue
    [338](338-artifacts-go-with-their-session.md).
  - The security suite records accordingly: a member of the channel reads the artifact in their
    own name, and is refused rather than not answered where they change it.

  Stored data to delete: artifacts, whose lists do not yet name the channels their versions were
  published in.
