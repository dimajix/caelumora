# 305 — A skill has no versions: a change overwrites what it said

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Context and knowledge
**Summary:** A skill keeps only its current text: every change overwrites what it said, nobody sees what changed, and an earlier version cannot be made current again.

## The problem

A skill (issue [22](22-skill-management.md), [SKILLS.md](../docs/SKILLS.md)) is a single document
that every change overwrites. Its owner edits it in the web UI, an agent rewrites it with
`update_skill`, and what it said before is gone. SKILLS.md says so on purpose - *"No versions.
What was loaded stands in the transcript word for word"* - but that only answers what a past turn
saw, not what the skill said, who changed it, nor how to get the old text back.

Wanted, as the maintainer put it:

- Every change of a skill makes a new version.
- The web UI shows the difference between two versions.
- An earlier version can be made the current one again.
- No branching: the history is linear.

## Why it matters

- **Agents write skills.** `update_skill` lets an agent rewrite a skill its owner owns; approval is
  all that stands between a model and the procedure a whole user's agents follow. A rewrite that
  slips past a hasty approver is final today - the same argument issue
  [256](256-workspace-versions.md) makes for workspaces.
- **A skill is tuned by trial.** One changes a step, watches how agents follow it, and wants to go
  back when it got worse - and to see what exactly was changed in between.
- **Who changed what** is recorded nowhere: `updated_at` is all a skill keeps.

## Considerations

### Generalise the artifact store?

Artifacts (issue [271](271-publishing-artifacts.md)) are the one thing in Caelumora that has
versions, so the obvious question is whether skills should be kept by the same machinery. There
are three ways to read it:

- **A. A skill is an artifact** (a Markdown artifact with a description). Rejected: almost every
  rule differs, and for a reason. An artifact's classification is fixed for all its versions and a
  republish with another label is refused - a skill is raised by `update_skill` and reclassified
  by its owner. An artifact's versions are published *in a channel* and readable through it, each
  with an access list of its own (issue 23) - a skill is shared as a whole and used with
  `UseSkill`, never through a channel. An artifact is opaque content that is downloaded - a skill
  is text the runtime reads in every turn that builds its catalogue or loads it.
- **B. Share the content storage**: old skill texts as content in an `ArtifactStorage`,
  generalised from `(ArtifactId, version)` to any key. Rejected: artifact storage is optional
  (`ArtifactManager::is_configured`) and configured per deployment; skill versions would then
  exist only where artifacts do. A skill text is at most 32 000 characters - nothing that needs a
  blob store, streaming or a part file -, and a second place to keep consistent with the document
  costs more than it saves.
- **C. Share the shape, not the store** (recommended). What artifacts and skills really have in
  common is the *history*: numbered from 1 without gaps, linear, never shortened but with the
  thing itself, each version recording who made it and when, one of them current. That is a small
  generic part of `caelumora-model` - the numbering, appending, finding a version, "the current
  one" - which `Artifact` and `Skill` both use with a version type of their own. The REST routes
  follow the same pattern (`…/{id}/versions`, `…/{id}/versions/{n}`), and the web UI shares the
  list of versions. Whether a generic type pays off for two users or the two histories only share
  their naming and routes is decided when it is built: the AGENTS.md rule is to align them, not
  necessarily to merge them.

Whichever is chosen, the differences that remain between artifact and skill versions should each
have a stated reason; those above do.

### What a version holds

- **The text**: `description` and `body` - what the model is shown and given. These are what
  agents and people change and what a diff is wanted for.
- **The label it was written under**: a turn raises the skill before it writes (`join_within`), so
  a version's text may be more confidential than an earlier one's. Each version keeps its
  classification; reading an old version asks clearance for *that* label.
- **Who and where**: the user, or the agent and the user it acted for, and for an agent its session
  and channel - so that the web UI can say "changed by agent X in channel Y".
- **Its digest**, so that a load recorded as knowledge can name the version it loaded (below).
- **Not versioned** (recommended): the name, the scope and the access list. They are the skill's
  identity and sharing, not its content; restoring an old name could collide with one taken since,
  and restoring an old access list would silently share or unshare. Their changes may be recorded
  in the history as entries without a text, if a full record of "every change" is wanted - to be
  decided.

### "Make an earlier version current"

- **By copying it as a new version** (recommended): restoring version 3 of 5 appends version 6
  with 3's text, made by whoever restored it. The history stays append-only and linear, the current
  version is always the newest, and the restore is itself recorded. Going "forward" again is
  restoring 5, which appends 7.
- **By moving a pointer**: `current = 3`, versions 4 and 5 stay. Cheaper, but the next edit is
  made from 3 while 4 and 5 still follow it - a branch in all but name, which the requirement rules
  out; and every reader has to know which version is current instead of taking the last.

A restore never lowers the classification: an old version written as `internal` restored into a
skill now `confidential` stays `confidential`. Lowering is the owner's act of its own (`Classify`),
as now. A restore is an `UpdateSkill` and, for an agent - if agents may restore at all -, approved
as `update_skill` is.

### Where the versions are kept

- **In the skill's document**, as an artifact keeps its versions. Simple and atomic, but every
  version carries up to 32 000 characters, and the catalogue and `list_skills` read every skill
  whole (issue [292](292-store-lists-read-everything.md)): a hundred versions of a few skills
  would make every turn's catalogue read megabytes.
- **Beside it** (recommended): the `Skill` document keeps the current text and its version number,
  so the catalogue, `load_skill` and the compaction read what they read now; the versions are
  documents of their own (`SkillVersion`: the skill's id and number, the text, its label, who,
  when), read only for the history and a diff, without permissions of their own - asked through the
  skill -, and purged with it. The skill and its newest version are written together by the
  `SkillManager`, version first, so that a crash leaves at most a version nobody points at.

### Diffs: in the web UI

Agreed with the maintainer's proposal. The server serves versions whole; the web UI computes the
difference. A diff is presentation - line or word level, side by side or inline -, the texts are
small, and the UI has to read both versions anyway. The web UI has no diff library yet; `diff`
(jsdiff) is the usual one. Should an agent one day be shown what changed (a `skill_history` tool),
the runtime would need a diff of its own - not part of this issue.

### Further points

- **Lost updates.** With numbered versions, a change can name the version it was made from (REST:
  the version in the request, `409` if the skill has moved on; `update_skill`: the version the
  agent loaded). Today, an agent's `update_skill` and the owner's edit in the web UI silently
  overwrite each other. Recommended as part of this issue, since versions make it cheap.
- **Knowledge names the version.** `KnowledgeSource::Skill { skill, digest }` would gain the
  version number, so that the web UI can link a load to exactly the text loaded.
- **The seed.** A skill of the system is written by the seed; whether applying a changed seed makes
  a new version or replaces the text is to be decided (recommended: a new version, made by the
  system).
- **Retention.** Versions are kept until the skill is purged. A limit can come later.
- **Artifacts.** Should artifacts get "make an earlier version current" too, for the same reason?
  With copying, that is publishing an old version's content again - not part of this issue, but
  the same name and route should be used if it comes.
- **Workspaces** (issue 256) are versioned differently - a tree, changed by commands that pass
  Caelumora by - and are not a case for the same machinery.

## Done when

- Every change of a skill's text - by its owner, by an agent, by the seed - makes a new version
  that records who made it, from where, and under which label.
- The web UI lists a skill's versions, shows the difference between two, and lets the owner make
  an earlier one current; REST offers the versions, the restore and the version a change is made
  from.
- A load recorded as knowledge names the version loaded.
- The security suite covers who may read and restore versions, and that an old version is shown
  only to whom is cleared for its label.
- SKILLS.md, REST-SERVER.md, STORAGE.md and WEB-UI.md describe it.

## History

- **2026-10-11** — Opened at the maintainer's request: versions of skills, a linear history,
  diffs in the web UI, and making an earlier version current; with the question whether the API of
  the artifact store should be generalised for it. Recommended above: share the shape of a history
  with artifacts, not their store (C); versions beside the skill document; restoring by copying as
  a new version; diffs computed in the web UI. Not decided yet.
