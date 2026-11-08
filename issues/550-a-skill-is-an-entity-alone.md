# 550 — A skill can only be an entity, and says too little of itself to be loaded by itself

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** A skill exists only as a `Skill` entity and cannot come from a Markdown file with a header, as other agent tools keep them, and it says nothing of itself but its name and one line, so nothing but the model's reading of that line can bring it into a turn without being asked.

## The problem

Three things, which build on each other:

1.  **Only an entity.** A skill ([SKILLS.md](../docs/SKILLS.md), issue [22](22-skill-management.md))
    is a `Skill` in the `EntityStore`. It is written through REST, the web UI, `create_skill`, or a
    seed, where its text is a string inside a YAML document
    (`examples/seed/skills/quiz-round.yaml`). A Markdown file - the form in which skills are written
    by hand, kept in a repository and passed between tools, `SKILL.md` among them - can be neither
    read nor loaded. SKILLS.md lists importing `SKILL.md` files as not decided; issue
    [279](279-agents-md-in-workspace.md) put off skills shipped with the code to that same question.
2.  **No metadata.** A skill has a name, a line of description, a body and a classification. There
    is no place for what a file like this one says of itself in its header:

    ```yaml
    ---
    name: citation-fixer
    version: 1.1.0
    description: |
      Audit and fix citation formatting across brain pages. Ensures every fact has
      an inline [Source: ...] citation matching the standard format. Extended in
      v0.25.1: scans for broken tweet/post references that lack actual URLs and
      resolves them via the host's X / Twitter API integration.
    triggers:
      - "fix citations"
      - "fix broken citations"
      - "citation audit"
      - "check citations"
      - "citation fixer"
    tools:
      - search
      - get_page
      - put_page
      - list_pages
    mutating: true
    ---
    ```

3.  **Loaded only when asked for.** A skill reaches a turn when a participant gives it (`/skill`) or
    when the model decides by the line in the catalogue to call `load_skill`. Nothing else in the
    system can bring it in, and a model that does not connect the task with the line does without.

Wanted, as the maintainer put it: skills that can also be loaded from traditional Markdown files,
metadata in the header of such a file, and the metadata put to sensible use, to load a skill
automatically where it fits.

## Why it matters

- **Skills are written elsewhere.** Whoever has skills in Markdown today has to take every one apart
  by hand to bring it here, and a skill that lives with a project's files cannot be kept there.
- **A procedure not loaded is a procedure not followed.** The catalogue line is all the model has to
  go by. What a skill is good for and when it applies does not fit there, and the more skills there
  are, the less a line each helps.

## Considerations

Nothing here is decided. The recommendations are proposals for the discussion.

### Where a file comes from

"Loaded from a file" can mean three different things, and they differ in what a skill then is:

- **A. The file is a form of the entity: read once.** The seed and an import (REST, the web UI) read
  a Markdown file and make a `Skill` of it; an export writes one back. There remains one kind of
  skill, with owner, scope, `UseSkill`, classification and trash as today. Issue 22 kept skills out
  of files because files "reintroduce a second source of truth and have no owner, no scope and no
  store-level check"; this keeps to that. But the file is dead once read: changing it changes
  nothing. Nothing new is needed for loading, for permissions or for the compaction.
- **B. The file stays the skill: read from a workspace.** A workspace's directory of skills, say
  `skills/<name>/SKILL.md` or a list of places as a parameter of the component, as
  `instruction_files` is for `AGENTS.md` (issue 279). The skills travel with the project's files and
  are edited as they are. Every question the entity answers is asked anew:
  - *Who may use it, and how it is classified:* as the workspace - where the channel may use the
    workspace, it may use the file, under the workspace's one classification. A header can raise
    nothing and lower nothing.
  - *Found by name:* a third place beside session, user and global, innermost of all? A workspace
    skill would then hide the owner's skill of its name.
  - *What was read:* `KnowledgeSource::Entity` has a `place` since issue
    [538](538-workspace-knowledge-without-a-place.md), so a file given is known by workspace, path
    and digest, and `workspace` already gives files again after a compaction. `load_skill` needs a
    second path to that.
  - *Trust:* whoever can write the file can put an instruction before the agent - a cloned
    repository, a command of the agent itself. As an `AGENTS.md` can today, but with automatic
    loading (below) this no longer waits for anyone to decide. Issue 22: "Whoever can put a skill in
    front of an agent can change what that agent does".
  - *Cost:* looking through a directory at every turn start, as issue 279 refused for nested files,
    and a catalogue in the system prompt that changes with the files, which breaks the prompt cache
    from there on (issue [537](537-prompt-parts-placed-anew-every-turn.md)).
- **C. The file stays the skill: read from a directory of the server.** A configured directory,
  re-read at startup or watched. It answers "shipped with the deployment", and has no owner but the
  system, like the seed. Against the seed it gains only that a change arrives; that can be had in A
  by letting the seed update (below).

**Recommendation:** A first and on its own, since the format and the metadata are needed by every
other way. Then B, as a source beside the entity behind one concept that `usable_skills` serves, if
that is what the maintainer means by "loaded"; C not.

**The seed in detail.** A seed file is YAML, read section by section, and a plugin is handed its
section as a `Value` (`SeedSection::seed`); every document is created by its `id`, once, and a
change in the file never arrives (`create_unless_taken`). A Markdown file has no `id` and the
kernel's reader knows no `.md`. Either a seed names the file
(`skills: [{ file: citation-fixer.md }]`), or the reader takes a `.md` as a document of the
section of its directory. The `id` comes from the header
or is derived from the name; only a derived one lets a hand-written file do without. Whether a
changed file should update a skill of the system is a change of what a seed is, and is the one use
`version` could have.

### The format

- **One owner.** Reading a file into a skill and writing one back belong to `caelumora-skills`,
  beside the entity, not to each of the seed, the routes and the workspace.
- **Foreign files have to load.** Fields unknown here are read past, not refused, or a file written
  for another tool is not usable - unlike the parameters of a component, which are refused
  (issue [515](515-parameters-a-component-does-not-take-are-ignored.md)): a component's parameters
  are all ours, a foreign header is expected to hold what is not known here. A deliberate
  difference, whose cost is that a misspelt `trigers` is read past as well, so what is left out is
  said in the import's answer, or logged. Not stored: nothing would read it.
- **Limits.** `Description` holds one line of 300 characters and `SkillBody` 32 000. The example's
  description is a block of four lines, so line breaks are folded as it is read. One that is too
  long is refused with its limit, not cut: a cut description misleads the catalogue.
- **No header:** `name` from the file's name, but a description is required, since the catalogue is
  of no use without it. A file that has none is refused.
- **What a file may not say.** Owner, access list and scope are never taken from a file. A
  `classification` in the header is taken only where nobody else says it (the seed), and the
  manager's check that the owner is cleared stays in any case.

### The fields, and what each is for

"Nothing without a user" holds for the metadata too: a field is stored only if something reads it.

| Field | Reading | Proposed use |
|---|---|---|
| `name`, `description` | as today | the same |
| `triggers` | phrases that say the skill applies | the automatic loading, below |
| `tools` | **the tools the skill needs**, not ones it grants | below |
| `mutating` | following it changes something | below |
| `version` | the author's label | shown; updates from a seed, if at all |

- **`tools` gives nothing.** A skill is instructions only (SKILLS.md, decided in issue 22), and some
  tools read such a list as a grant. Here it must not be: what a skill may do stays bounded by the
  agent's tools and their approval. So it can only say what is missing. Names are the difficulty:
  a foreign file's `get_page` exists nowhere here, and an MCP tool is called
  `{component_id}__{name}`. Hiding a skill whose tools are missing would then hide every foreign
  one. **Recommendation:** it stays in the catalogue and can be loaded, but is not loaded
  automatically while a tool is missing.
- **`mutating` only ever makes things stricter.** It is the author's claim, so `false` relaxes
  nothing: the tools ask their approval all the same. **Recommendation:** a skill that says `true`,
  or says nothing, is offered rather than given (option 3 below).
- **`version` is not the history of issue [305](305-skill-versions.md).** That is a linear count
  made by every change; this is what the author calls it. If 305 is built, the two should not
  share a name in the code. SKILLS.md's "No versions" says something else, and has to be reworded.

Beyond the header, the fields reach every writer: `SkillUpdateEvent`, the routes (one per field
changed), `create_skill` and `update_skill`, the web UI's page, the wire types. Stored skills take
defaults; none has to be deleted. Whether they are one field `metadata` or several is decided with
the interface.

### Automatic loading

- **1. Triggers in the catalogue.** The line says when the skill applies, and the model decides. No
  new mechanism, works in every language, stays within the catalogue's cache behaviour. It is not
  automatic, and costs tokens for every skill in every call.
- **2. Matched and given.** The turn's input is compared with the phrases, and a match is given
  before the model's first call. Predictable and needing no model. But phrases are bound to their
  language, so "Zitate reparieren" finds nothing, and a mention in passing loads up to 32 000
  characters into the transcript.
- **3. Matched and offered.** A match gives a short line in the turn - this skill seems to apply,
  load it with `load_skill` - and the model decides. It costs a line where it is wrong, and leaves
  the last word with what understands the task.
- **4. Judged by a model or by embeddings** on every turn: finds what phrases miss, and costs a call
  or an index per turn. Nothing of the kind exists for skills; issue
  [59](59-retrieval-of-notes.md) asks the same for notes.
- **Preloading by a personality**, which SKILLS.md lists as not decided, is a different thing:
  always, whatever is said. It is not part of this issue unless wanted.

**Recommendation:** 1 and 3 together; 2 only for a skill that is not `mutating`, whose tools are
there, and that is an entity, not a file of a workspace; 4 later, if phrases prove too little.

What holds for all of them:

- **The same permissions.** Only what `usable_skills` returns is matched, so that nothing is loaded
  that `load_skill` would not load: `UseSkill` for the owner, and the classification flowing into
  the channel.
- **No facet for it yet.** A `PromptProvider` is given a `Situation` and writes no transcript, and a
  part at `Given` is taken back when left out next turn, which is wrong for a skill given once. A
  `MessageProcessor` sees messages only. Option 2 needs the agent's turn to ask its components at
  its start, with `Turn::give` in hand: a new facet, whose interface is to be put before the
  maintainer. Option 3 can be a part at `BeforeTurnInput`: a `Situation` reaches the agent's
  channel state, and the tools of the turn (`tool_sets`).
- **What is matched** is the input of the turn, from whichever participant - a person or an agent
  alike -, not tool results: those are what a stranger's text comes in by. Case and spacing are
  ignored, whole words only.
- **Not again.** What stands in the transcript is not given again: `Turn::give` already answers so
  (issue [541](541-skill-loaded-again-though-given.md)). A turn takes a limited number of skills,
  given with the limit of what is given again in a compaction in mind (issue
  [542](542-what-is-given-again-unbounded.md)).
- **In whose name.** Given automatically is neither a participant's gift nor the model's call; the
  transcript and the channel have to say which, as issue 539 frames what is given.
- **Agents write triggers too.** A skill written by `create_skill` with a broad trigger puts an
  instruction in front of every later turn of the owner's agents. The approver is shown the
  metadata with the skill, and the tool's text says what a trigger is for.

## Done when

To be made precise once the options above are decided. At least:

- A Markdown file with a header can be read into a skill, in one place, and foreign fields do not
  stop it; a seed and a user can bring one in.
- The agreed metadata is stored, can be changed by whoever may change the skill, and is shown.
- `tools` grants nothing, and `mutating: false` relaxes nothing, and a test holds both.
- A skill whose triggers fit the input reaches the turn by the agreed way, only if `load_skill`
  would have loaded it, and not a second time; a test in the security suite holds it.
- SKILLS.md describes it, its *Not decided yet* and *No versions* as they then stand; COMPONENTS.md,
  CONFIGURATION.md, REST-SERVER.md, WEB-UI.md and `examples/seed/README.md` where they are touched.

## History

- 2026-11-08: Opened at the maintainer's request: skills from traditional Markdown files as well,
  metadata in a file's header, used to load a skill automatically. Options and recommendations
  written against `0f542c31`; nothing decided yet.
