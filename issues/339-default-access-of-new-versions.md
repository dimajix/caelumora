# 339 — Whom a new version of an artifact reaches cannot be set, nor a version's list changed

**Status:** open
**Area:** Identity and access management

## The problem

With issue [316](316-store-checks-artifact-without-versions.md), an artifact and each of its
versions carry an access list of their own, as directories and files do in HDFS and ADLS: reading a
version needs reading the artifact *and* the version. Publishing a version gives it reading by the
participants of the channel it is published in, and nothing else; a version's list cannot be
changed afterwards. Sharing the artifact (`ArtifactManager::set_access`) may hand its entries to the
versions there are, but not to those published later.

So an owner who shared an artifact with a team has to share it again after every new version, and
cannot restrict or widen whom a single version reaches.

## Why it matters

- Whom a result reaches should be decided once, where the artifact is shared, not after every
  version.
- That a version does not reach more people at once than intended - a draft published to a team
  the artifact is shared with - is to be ensured by what a new version is given, not by luck.

## Considerations

- **A default list on the artifact (`default_acl`)**, as HDFS and ADLS keep one on a directory for
  its children: copied onto a version when it is made, besides the reading of the channel it is
  published in, and of no meaning to any check otherwise. Its entries grant reading alone, as a
  version's do.
- **Changing a version's list** by its owner, as the artifact's is changed (`UpdateArtifact`);
  whom it names must be readable to them (`require_nameable`, issue 286).
- **Sharing with the defaults**: `set_access` hands its new entries to the default list as well,
  if asked - as `setfacl -d` does.
- **The REST API and the web UI** (issue [291](291-share-artifacts-in-the-web-ui.md)'s page of an
  artifact) show and change the default list and the lists of versions.

## Done when

- An artifact carries a default list, which a new version is given, and its owner can change it.
- Its owner can change the list of a version.
- The web UI shows and changes both.

## History

- **2026-10-13** — Opened while deciding issue 316 with the maintainer, as its follow-up: the
  model of an artifact's and a version's lists, both of which reading a version needs, follows
  HDFS and ADLS; their default list for children is this issue.
