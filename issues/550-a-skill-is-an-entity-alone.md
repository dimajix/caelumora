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

## Decision

Decided with the maintainer on 2026-11-08:

1.  **Files come from two places:** a seed, where a file becomes a `Skill` once (A above), and a
    workspace, where the file stays the skill (B). No directory of the server (C), no import or
    export by REST or the web UI.
2.  **Automatic loading starts as a hint:** a skill that fits is named to the model, which decides
    (option 3). Nothing is given without the model or a participant asking for it.
3.  **A decision model judges what fits**, not a text match. A literal phrase is what the catalogue
    already serves; what a hint is for is the paraphrase and the other language. `triggers` are what
    opts a skill in, and part of what the model is told of it. Without a decision model there are no
    hints, and the catalogue, which names the triggers, stays.
4.  **It is measured first**, because the measurement in LLM-MODELS.md is of choosing among agents.
5.  **Decision models become the third kind of the models the platform offers**, beside language
    and image models, because the library of `skills` may not link `caelumora-decision`, which
    links the kernel. `decision_policy` is moved onto it too, so that there is one way to ask one.

## The plan

Proposed for the maintainer's review. Each step is reviewed and committed on its own, on the branch
`issue-550-skills-from-files`. The patterns followed: `image_model` and `ImageModelClient` for a
kind of model (issue 525), `image_generation` for a component that needs one, `decision_policy` for
how a decision model is asked, `instruction_files` for what is looked up in a workspace (issue 279),
`SeedSection` for what a plugin seeds, and the `Text<Rule>` values for limits.

### Step 1: the measurement

`work/decision_models/probe.py`, extended: which of about a dozen skills, by name, description and
triggers, fit messages in English and German, literal and paraphrased, with messages no skill fits
among them, and with the triggers left out for comparison. For the models LLM-MODELS.md lists. It
says whether a question per skill (`noul`) or one over all (`choice`) works better, which threshold
to take, and what the triggers add. Assumed below: `noul`, since several skills can fit. Results go
to `work/decision_models/RESULTS.md` and LLM-MODELS.md. If no model tells fitting from not fitting,
the plan goes back to the maintainer before step 6.

### Step 2: decision models as a kind of model (refactoring)

```rust
// caelumora_api::decision - moved from caelumora_decision::client
pub enum Question { Choice { .. }, Noul { .. } }
pub enum Answer { Choice { .. }, Noul { .. } }
pub const MAX_OPTIONS: usize = 26;
pub const MAX_QUESTIONS: usize = 64;

/// The facet through which one decision model the platform offers is asked, built for whoever
/// asks it alone.
#[async_trait]
pub trait DecisionModelClient: Send + Sync + Debug {
    /// The model's name, as the configuration gives it.
    fn name(&self) -> &str;
    /// Whether what is classified `classification` may be sent to the model.
    fn may_receive(&self, initiator: ActorId, classification: &Classification) -> bool;
    /// The answers to `questions` about `state`, which is classified `classification`, by the
    /// name of each. `ModelError::Unfit`, nothing sent, for what it may not receive.
    async fn decide(
        &self,
        initiator: ActorId,
        classification: &Classification,
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Answer>>;
}

// caelumora_model
/// A decision model the platform offers: one entry of `plugins.model.decision`.
#[async_trait]
pub trait DecisionModelProvider: Send + Sync + Debug {
    /// What its server is cleared for.
    fn destination(&self) -> &Classification;
    /// The answers to `questions` about `state`, its credential resolved for `asking`.
    async fn decide(
        &self,
        asking: &SecurityContext,
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Answer>>;
}

impl ModelManager {
    /// The client of the decision model `name` - the one there is where `None` -, for whoever
    /// `acting` acts for alone.
    pub fn decision_client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<Arc<dyn DecisionModelClient>>;
}
```

- **The plugin `caelumora-model`** gets the section `plugins.model.decision`, by name, and the kind
  `decision_model` (`name`), which provides the `DecisionModelClient`, as `image_model` does.
  `ModelManager` becomes public for the one method above, and holds the `ConfidentialityPolicy`.
- **`caelumora-decision`** brings the backend `systemone` (`endpoint`, `model`, `keep_alive`,
  `destination`), and depends on `caelumora-model`, as `caelumora-model-ollama` does.
  `DecisionClient` becomes that backend, and `DecisionError` goes: a call answers `ModelError`.
- **`decision_policy`** names its model by `name` and builds its client as `image_model` does (a
  channel is built in an `Acting` given once). `endpoint`, `model`, `keep_alive` and `destination`
  leave its parameters; `threshold`, `max_responders` and how senders are marked stay.
- **`destination` moves to the model**, so that it is said once and by the operator, for every
  user of the model. So nothing in the policy's parameters says where anything goes any more, and
  `operator_only` is taken off it, its reason gone.
- **Two named differences.** The provider is handed who asks, where a language model's is not: a
  hosted decision server takes a credential, which `caelumora-model-ollama` refuses since nothing
  tells it who asks. And the policy now asks in the context of the channel's owner, not of the event
  routed; `may_publish` does not depend on the context, so only which credential is resolved
  changes, and that for the better: not the sender's.

Files: `caelumora-api/src/decision.rs`,
`caelumora-model/src/{lib,manager,plugin,decision_model}.rs`,
`caelumora-decision/src/{client,policy,plugin}.rs` and its `Cargo.toml`,
`examples/seed/session_templates/team-session.yaml`, `examples/config.yaml`. Tests: as those of
`image_model` for the kind; those of the policy against the fake server stay; a client refuses what
may not reach its destination; the policy's refusal, now through the client. Documents: CHANNELS.md,
COMPONENTS.md, CONFIGURATION.md, LLM-MODELS.md, ARCHITECTURE.md, CRATES.md. Stored data: templates
and channels with a `decision_policy` of the old parameters have to be deleted.

### Step 3: metadata, and a skill as a file

```rust
// caelumora_skills::skill
/// What a skill says of itself beside its text: all a header adds to a name and a description.
pub struct SkillMetadata {
    /// Phrases that say it applies; without any it is never hinted at.
    pub triggers: Vec<SkillTrigger>,
    /// The tools it needs, by name. Grants nothing: it says what is missing.
    pub tools: Vec<Name>,
    /// Whether following it changes something. Told, never relaxes anything.
    pub mutating: bool,
    /// What its author calls this state of it; no history (issue 305).
    pub version: Option<SkillVersion>,
}
pub type SkillTrigger = Text<SkillTriggerRule>; // one line, 120 characters
pub type SkillVersion = Text<SkillVersionRule>; // one line, 40 characters

// caelumora_skills::document (feature `api`)
/// A skill as a Markdown file holds it, its header and its text: what a seed and a workspace hold.
pub struct SkillDocument {
    pub name: Name,
    pub description: Description,
    pub metadata: SkillMetadata,
    /// As its header says; only a seed takes it.
    pub classification: Option<Classification>,
    pub body: SkillBody,
    /// The keys of its header that mean nothing here, for whoever reads it to say so.
    pub ignored: Vec<String>,
}

impl SkillDocument {
    /// The name of a skill lying at `path` of a directory of skills, `<name>.md` or
    /// `<name>/SKILL.md`; `None` for anything else that lies there.
    pub fn name_at(path: &str) -> Option<&str>;
    /// The skill `markdown` holds, `named` unless its header names it. The line breaks of its
    /// description are folded; a key not known is read past.
    pub fn parse(markdown: &str, named: &str) -> Result<Self, SkillDocumentError>;
}

// Changed
impl SkillManager { pub async fn annotate(&self, ctx, id, metadata) -> SkillResult<Skill>; }
trait SkillWriter { async fn annotate(&self, id: SkillId, metadata: SkillMetadata)
    -> SkillResult<Skill>; }
```

`Skill` and `SkillSnapshot` get a `metadata`, which `Skill::new`, `SkillManager::create` and
`SkillWriter::create` take; `SkillUpdateEvent::MetadataChanged`. REST: `PUT /skills/{id}/metadata`
as one route per thing changed, `metadata` in `CreateSkillRequest` and `SkillResponse`. At most 16
triggers and 32 tools. `create_skill` and `update_skill` take `triggers`, `tools` and `mutating`,
and the approver is shown them. The header is read by `caelumora_domain::config::parse_yaml`, which
quotes nothing of the file in its errors. The catalogue's line names the triggers, whether the skill
changes things, and the tools it needs that the turn does not offer (`Situation::tool_sets`; a tool
of an MCP server by the part after `__`). The web UI's `SkillPage` shows and edits the metadata.

Tests: `parse` with the example of this issue, without a header, with foreign keys, with too long a
description; the manager and the routes for `annotate`; `update_skill` asks as for the text; the
catalogue. Documents: SKILLS.md (the header, and *No versions* reworded), REST-SERVER.md, WEB-UI.md,
COMPONENTS.md. Stored data: none; `metadata` takes its default.

### Step 4: Markdown files in a seed

```rust
// caelumora_kernel::seed
/// A file of the directory of a section, other than the seed files: its path below that directory
/// and what it holds.
pub struct SeedFile { pub path: PathBuf, pub content: String }

trait SeedSection {
    async fn seed(&self, section: Value) -> Result<()>;
    /// Puts what `files`, the Markdown files below the section's directory, hold into the plugin's
    /// stores, as `seed` does; a section that takes none leaves them alone.
    async fn seed_files(&self, files: Vec<SeedFile>) -> Result<()> { Ok(()) }
}

// caelumora_skills::skill
impl SkillId { /// The id of the skill a seed's file names `name`, the same at every start.
    pub fn of_seed_file(name: &Name) -> Self; }
```

A seed directory's subdirectory that a plugin's section claims is that section's directory; a
README beside them is in none, as today. The kernel reads the files and knows no skill.
`SeededSkills::seed_files` keeps those of `SkillDocument::name_at`, parses them and makes a skill of
the system in the global scope, classified as the header says or `CHANNEL_DEFAULT`, by
`create_unless_taken`; a file that cannot be parsed is an error at startup, naming the file, and an
ignored key a `warn!`. The id comes from the name (`Uuid::new_v5`), since a file has none, and
without an id a skill deleted on purpose would come back. So renaming a skill in its file makes a
new one, and a changed file does not arrive, as with every seed document. `quiz-round` becomes the
example's Markdown file, with triggers.

Tests: `seed.rs` for what goes to which section, and that a README is none; the plugin for a skill
from a file, seeding twice, one deleted staying deleted. Documents: CONFIGURATION.md, SKILLS.md,
`examples/seed/README.md`. Stored data: the skill `quiz-round` of a seeded store has another id now
and has to be deleted.

### Step 5: skills in a workspace

```rust
// caelumora_skills::component::usable (pub(crate))
/// A skill the work may load where it is, from wherever it comes.
struct UsableSkill { document: SkillDocument, classification: Classification, origin: SkillOrigin }

enum SkillOrigin {
    Stored(SkillId),
    /// A file of the channel's workspace, with the digest of what was read.
    Workspace { workspace: WorkspaceId, path: WorkspacePath, digest: Digest },
}

/// The directories of a workspace skills are looked for in, as `AGENTS.md` and `CLAUDE.md` are.
const WORKSPACE_SKILL_DIRECTORIES: [&str; 2] = [".agents/skills", ".claude/skills"];
const MAX_WORKSPACE_SKILLS: usize = 32;

/// The skills the work may load where it is: those of the `SkillReader`, then those of the
/// workspace of a name that is not taken.
async fn usable_skills(situation: &Situation<'_>) -> SkillResult<Vec<UsableSkill>>;
```

- **Everything of the component goes through it**: the catalogue, `list_skills`, `load_skill`,
  `/skill` and `/skills`. `SkillReader` stays as it is and answers for the store alone: the
  files are read through the `WorkspaceReader`, in the work's name, as the workspace's own prompt
  part does, and the kernel has checked that the channel may use the workspace.
- **A skill of the store wins over a file of its name**, so that a repository cannot displace what
  a user set up. A file is classified as its workspace, and its header's classification is not read.
- **A file read fails alone:** a storage out of reach, a file that cannot be parsed or is too long
  leave that skill out with a `warn!` and do not cost the turn (issue 279).
- **What was read** is `KnowledgeSource::within::<Workspace>(id, path, digest)` with the digest of
  the whole file, as `read_file` records it. So `workspace` gives it again after a compaction, and
  `write_file` knows it was read. `update_skill` refuses one: it is changed as a file.
- **The catalogue names them in a list of their own**, saying they were written by whoever wrote the
  workspace's files, since that text now stands in the prompt; `AGENTS.md` only ever had its name
  there.
- **Reading a text file whole** is in `caelumora-tool-workspace` (`files.rs`, private); it moves to
  `caelumora_api::terminal` before this second use.
- **The directories are a constant**, not a parameter as `instruction_files` is: `skill_hints` needs
  them too, and nobody needs to set them yet.

What it costs: the directories are listed and the files read at the start of every turn, at most 32.
Nothing is cached until a measurement on a remote storage asks for it (`FileEntry::modified` would
serve as the key).

Tests: found in both forms and both directories; a store's skill wins; a file refused or out of
reach is left out; `load_skill` and `/skill` give it, known by place and digest, and once only; a
channel that may not use the workspace sees none, in the security suite; given again after a
compaction. Documents: SKILLS.md, WORKSPACE.md, COMPONENTS.md, CONFIDENTIALITY.md if the wording
there is touched. Stored data: none.

### Step 6: the hint

A new kind, since a component has one prompt part and this one moves with every turn:

```rust
// caelumora_skills::component::hints; kind `skill_hints`
struct SkillHintsParams {
    /// Where the hint goes: `end` unless configured.
    position: Option<PromptPosition>,
    /// How probable it must be that a skill fits: 0.5 unless step 1 says otherwise.
    threshold: f64,
    /// `2` unless configured.
    max_hints: usize,
}

// caelumora_domain::agent::channel_state
impl AgentChannelState {
    /// What others said since the agent last spoke: what a turn answers.
    pub fn heard_since_it_spoke(&self) -> Vec<&Message>;
    /// Whether what `source` names stands given in the transcript (what `given` asks, issue 541).
    pub fn stands_given(&self, source: &KnowledgeSource) -> bool;
}
```

`PromptProvider`, built with `.needs::<dyn DecisionModelClient>()`, refused without one, as
`image_generation` is. `get_prompt`, once at the start of a turn:

1.  The usable skills that have triggers, whose tools the turn offers, that do not stand given, and
    that may reach the model (`may_receive`).
2.  The state: `heard_since_it_spoke`, its last 1,024 tokens, classified as it is joined. If it may
    not reach the model, or there is nothing heard, no hint. Whoever said it, person or agent;
    tool results are not heard.
3.  One `noul` per skill, its name, description and triggers (`prompts.yaml`). Beyond 64, or beyond
    the model's 8,192 tokens, no hint and a `warn!`, as `decision_policy` answers too many.
4.  Those at or above `threshold`, the most probable first, at most `max_hints`, named in a part
    classified as they are joined, which says that they may fit and how to load one.

Whatever fails - no answer in time, a model still loading, an answer not understood - gives no
hint, with a `warn!` that holds nothing said, and never costs the turn. The part is empty otherwise.
Which agents get it is their personality's, by the component; the example's `Coder` gets it.

Tests: with a scripted `DecisionModelClient`, what is asked, the threshold and limit, the filters of
1, what fails; in the security suite, that nothing goes to a model it may not reach and only what
`load_skill` would load is named. Documents: SKILLS.md (and its *Not decided yet*), COMPONENTS.md,
LLM-MODELS.md, AGENT-TURN.md if a part is listed there.

### Not part of this issue

Giving a skill without the model asking, a text match, an import or export, a changed seed file
updating a skill, asking in several calls beyond what one call holds, preloading by a personality.

## Done when

- A decision model is a model the platform offers, and `decision_policy` asks one as such.
- A Markdown file with a header is read into a skill in one place, and foreign fields do not stop
  it; a seed brings it in once, and in a workspace it can be loaded as it stands.
- The metadata is stored, changed by whoever may change the skill, and shown.
- `tools` grants nothing, `mutating: false` relaxes nothing, and a file's header sets no
  classification in a workspace; a test holds each.
- Where a decision model is configured, a skill whose triggers fit what was said is named to the
  model, only if `load_skill` would load it, and not while it stands given; a test in the security
  suite holds it. Without one, or when it fails, the turn goes on without a hint.
- How well it chooses is measured and in LLM-MODELS.md.
- The documents named in the steps describe it.

## History

- 2026-11-08: Opened at the maintainer's request: skills from traditional Markdown files as well,
  metadata in a file's header, used to load a skill automatically. Options and recommendations
  written against `0f542c31`; nothing decided yet.
- 2026-11-08: Decided with the maintainer (*Decision*): files from a seed and from a workspace, a
  hint to the model as the start of automatic loading, and a decision model to judge what fits,
  measured first and reached as a kind of model of its own. *The plan* written against `2e5b1de6`,
  for the maintainer's review.
