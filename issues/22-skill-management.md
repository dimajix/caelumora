# 22 — Managing skills, and who is allowed to use them

**Status:** in progress
**Area:** Context & knowledge

## The problem

There is no notion of a *skill* anywhere in the workspace. What an agent knows how to do is fixed at
build time by its `Personality.components`: a `static_system_prompt_provider` carries the
instructions, `ToolProvider` components carry the capabilities, and both are decided when the
personality is authored. There is no unit in between — a named, self-contained piece of procedural
knowledge ("how we write a release note", "how this Postgres schema is migrated") that exists
independently of any one personality, can be authored and updated on its own, and is pulled into a
turn only when it is actually relevant.

Two things are missing, and the second is the harder one:

1.  **Management.** Where a skill lives, what it consists of, how it is created, updated, listed and
    versioned.
2.  **Access control.** A skill is not public knowledge. Which agent may load which skill — and on
    whose behalf — has to be an enforced decision, not a convention. `ActionType` has no variant for
    it today.

## Why it matters

Without skills, every capability has to be baked into a personality, which means the instruction set
of an agent grows monotonically and every agent that needs a procedure carries it in its system
prompt for the whole session. That is the same pressure that [16](16-context-compression.md)
describes from the other end. Loading instructions on demand is the cheap half of the answer.

The access-control half matters for a different reason. A skill is an instruction that an agent will
follow. Whoever can put a skill in front of an agent can change what that agent does — for every
user that agent serves. If skills may additionally carry tools, a skill is executable content, and
`PutSkill` becomes a privilege-escalation path rather than an authoring operation. This has to be
decided before the first skill exists, not after, because retrofitting a permission boundary around
a mechanism people already rely on means taking capability away from them.

## Considerations

- **Where skills live.** `EntityStore` is the obvious home: it is where `Personality`,
  `SessionTemplate` and, since the registry moved, `ComponentDescriptor` already live, and it brings
  `Metadata`, `SecurityAttributes` (hence [11](11-agent-ownership.md)) and per-call gating on a
  `SecurityContext` for free. Files on disk would be easier to author by hand but reintroduce a
  second source of truth and have no owner, no scope and no store-level check.
- **How a skill reaches the agent.** The composition point already exists: `StateContextProvider`
  merges any number of `SystemPromptProvider`s into the turn's context. A `skill_provider` component
  that implements `SystemPromptProvider` for the *catalogue* (name and one-line description of every
  skill the caller may load) and `ToolProvider` for a `load_skill` tool that fetches the body on
  demand fits the existing architecture without a new pipeline stage — and progressive disclosure is
  exactly what keeps the always-on cost at one line per skill. The alternative — a new component
  facet, or a fourth stage in the cognitive cycle — buys nothing that these two traits do not
  already give.
- **Two gates, not one.** Authoring (`PutSkill`, `DeleteSkill`) and using (loading a skill into a
  turn) are different decisions with different subjects, and collapsing them into one `ActionType`
  will be wrong in one direction or the other. The using side is the novel one: it is checked
  against the agent *and* the user it acts for, which is precisely the unresolved question in
  [12](12-user-and-agent-permissions.md). Until that rule exists, `UseSkill` has no defensible
  semantics — this issue is blocked on 12 for the runtime half, not for the management half.
- **The catalogue must be filtered by the same check that gates loading.** Listing a skill the
  caller may not load leaks its existence, and worse, invites the model to attempt it and then
  explain the refusal to the user — see [15](15-disallowed-tool-errors-to-llm.md) for why a refusal
  the model cannot act on is its own problem.
- **May a skill carry executable content?** Instructions-only skills are checkable: the worst case
  is bad advice, and the existing tool permissions still bound what follows from it. Skills that
  bundle `ComponentDescriptor`s or tools are far more useful and turn skill authoring into a way to
  inject components into somebody else's agent. If this is allowed at all, the components a skill
  brings must be subject to the same `CallTool` and approval path as any other tool
  ([13](13-approval-modes.md), [14](14-approval-privilege.md)) — a skill must not be a way to arrive
  at an already-approved capability.
- **Scope and sharing.** Global, owner-scoped and session-scoped skills are all plausible, and the
  answer overlaps heavily with [18](18-knowledge-sharing.md) and [19](19-memory-tiers.md): a skill
  an agent derives itself is one concrete shape "knowledge that outlives the session" could take.
  Skills should not become a second, parallel memory mechanism with its own lifetime rules;
  whichever tier model 19 settles on should cover them.
- **Auditability.** Which skills were loaded into a turn is part of what the agent saw when it
  answered, so it belongs to the same reconstruction problem as
  [06](06-context-in-message-for-auditability.md), and the load itself is a security decision that
  belongs in the recorded event stream ([05](05-postgres-event-recorder.md)).
- **Enforcement is vacuous today.** With `DummySecurityManager` approving everything and the REST
  server fabricating its own `SecurityContext` ([10](10-user-authentication.md)), any check added
  here is structure without effect. That is an argument for defining the `ActionType` variants and
  the call sites now — so that authentication, when it lands, has something to enforce — not for
  postponing them.
- **Naming and collisions.** Several skills with the same name from different scopes is the normal
  case, not the exception. `mcp_tool` already solved the analogous problem by namespacing remote
  tools as `"{component_id}__{remote_tool_name}"`; whatever is chosen here should not be a third
  convention.

## Decision

Decided with the maintainer on 2026-10-09, after [269](269-agent-commands.md) made room for it:

1.  **A skill is an entity** in the `EntityStore`, built as the workspace is (issue 253): a
    `ScopedName` in the global, a user's or a session's scope - no channel scope -, a one-line
    `description` for the catalogue, a Markdown `body`, one `classification`,
    `SecurityAttributes` with owner and contributors, `deleted`. Instructions only: no executable
    content, no tools - a skill tells the model how to do something with the tools it has.
2.  **Two gates.** Authoring is `GetSkill`, `ListSkills`, `CreateSkill`, `UpdateSkill`,
    `DeleteSkill`, as for every entity. Loading one into a turn is `UseSkill`, an action of its
    own, asked in the turn's context - the agent for its owner, whoever asked as initiator (issue
    255) - together with whether the skill's classification may flow into the channel
    (`Publish`, as a workspace's is asked when it is used). The catalogue is filtered by the same
    two questions, so that a skill the agent may not load is not listed.
3.  **A skill reaches the model through the agent's transcript alone**, never through the system
    prompt - the prompt cache, the digest record, the branches - and never as a message of the
    channel's history, which would hand its text to members who may not read it. Two ways in,
    one rendering: the model calls `load_skill` and gets the body as a tool result, plain text
    under a title, with the content hidden from the channel (`CALL_ONLY`); or a user types
    `/skill @agent <name>` and the agent puts the body into its transcript as a message in the
    user's name, through the command mechanism of 269. Both record the read as knowledge.
4.  **The catalogue** - name and description of every skill the agent may load here - is a prompt
    part, in the system prompt by default (`position`): it changes rarely, and the model has to
    know what it could load.
5.  **One component, `skills`**, as `notes` is one: the catalogue (`PromptProvider`),
    `list_skills`, `load_skill`, `create_skill`, `update_skill` (`ToolProvider`), `/skill` and
    `/skills` (`AgentCommandProvider`). An agent without the component knows no skills.
6.  **Agents may create skills**, within guard rails: owned by the agent's owner, in the session's
    scope or the owner's - never global -, classified as the turn's label (issue 168), with
    approval by default; `update_skill` reaches the owner's own skills alone, which `UpdateSkill`
    ensures.
7.  **Nothing of a skill is a message of the channel.** The `/skill` command is recorded as every
    command of an agent is (`HistoryEntry::CommandExecuted`); the body stands in the agent's
    transcript and its own history, as what it was given.

## Plan

Written against the code as of `2c6dcb3`. The model first, then the runtime without authoring
tools, then authoring, then REST, the web UI and the seed, then the documents.

### A. What the code shows

- **The workspace is the template** for an entity of this shape: `caelumora_model::workspace`
  (`Workspace`, `WorkspaceUpdateEvent`, `Stored::validate` for the scope rule), the store trait's
  ten methods per kind with `store::access::WORKSPACES` for the checks, `EntityKind::Workspace`,
  `GlobalEvent::Workspace…`, `ActionType::…Workspace`; `caelumora_runtime::workspace::
  WorkspaceManager` as the one writer - stored first, then announced, under a lock for names -,
  held by the `RuntimeManager` and reached from the views (`Views::workspace_manager`);
  `view::WorkspaceView` with `owned` / `in_channel`, `snapshot`, `may_change`; `scoped.rs`
  deleting, restoring and purging what lies in a session's scope with it; REST
  `routes/workspaces.rs` with one route per field changed; the web UI's `WorkspacePage`,
  `NewWorkspaceDialog` and the sidebar's section following `Workspace…` events. Personalities and
  templates, by contrast, have no manager and no REST: they come from the seed.
- **A multi-facet component**: `component_impl/notes.rs` - `ToolProvider` with its tools' texts,
  `PromptProvider` with a `BeforeTurnInput` part rendered by `render_sections`, knowledge recorded
  per read (`KnowledgeRef`, `KnowledgeSource::Notes`), results classified (`ToolOutput {
  value, classification }`), reads through `turn.session()` (`SessionView::channel_named`).
  `agent_memory` asks the policy per entry with `may_copy_information` from a `Place`; the
  workspace asks `may_publish` of its classification into the channel's - the fitting question
  for something that has no channel of its own.
- **Finding by name from a place**: `scope::find_by_name(scoped, name, place, user)` - the
  innermost of what is visible from the place's scope and the user's own; `Scoped` is
  implemented by every kind with a `ScopedName`.
- **Into the transcript**: `AgentChannelState::said(message)` records a message into the
  transcript and the history at once, with the turn's `ContextRef` - "produced by the agent" -;
  between turns without one, "as heard would be". `take_in` perceives what others said through
  the perceive processors. There is no way yet to give an agent a message to read in a turn
  without marking it as produced by the agent.
- **Text with a limit** is a `Text<Rule>` (`value/text.rs`): `Description` 300 characters,
  `Briefing` 16 000.
- **Digests** have a scheme per kind in `digest.rs`; `digest::prompt(text)` digests a prompt's
  text.
- **The seed** (`examples/seed.yaml`, `seed.rs`) holds personalities and templates of the system.
- **From 269**: `AgentCommandProvider::execute(name, args, turn)`; `CommandRequest::issuer` in
  `TurnState::command`; a command turn is dropped when the command fails, and since issue 260 a
  message recorded is recorded at once - so a command gives the agent something to read only once
  nothing can fail any more.

### B. Step 1 - the model (`caelumora-model`)

1.  `skill.rs`: `define_id!(SkillId)`; `Skill { id, name: ScopedName, description: Description,
    body: SkillBody, classification, security_attributes, created_at, updated_at, deleted }`;
    `SkillBody = Text<SkillBodyRule>` with 32 000 characters, in `value/text.rs`; `Skill::new(name,
    description, body, classification, owner: ActorId)`; `SkillUpdateEvent { NameChanged,
    DescriptionChanged, BodyChanged, ClassificationChanged }`; `Stored::validate`: the scope is the
    global one, a session's, or the owner's own user scope - a skill of the system (the seed's)
    lies in the global scope; `Deletable`, `Secured`, `Scoped`.
2.  `EntityKind::Skill`; the store trait: `create_skill`, `get_skill`, `update_skill`,
    `delete_skill`, `list_skills`, `restore_skill`, `purge_skill`, `get_deleted_skill`,
    `list_deleted_skills`, `find_skill_by_name`; `store::access::SKILLS`; the memory and the JSON
    store; the store suite's tests for the kind, as for workspaces.
3.  `ActionType`: `GetSkill(a)`, `ListSkills`, `CreateSkill(a)`, `UpdateSkill(a)`,
    `DeleteSkill(a)` under the ownership policy as every entity's; `UseSkill(a)`: loading it into
    a turn, granted as reading is - the principal may read it, or the caller (an agent)
    contributes to it - so that under this policy use and reading coincide while the two stay
    apart for another policy and for the record. `describe` names them.
4.  `GlobalEvent::SkillCreated(SkillId)`, `SkillUpdated(SkillId, SkillUpdateEvent)`,
    `SkillDeleted`, `SkillRestored`, published with the skill's attributes.
5.  `KnowledgeSource::Skill { skill: SkillId, digest: Digest }`: the body as it was read, digested
    with a scheme of its own (`digest::skill`).
6.  `AgentChannelState::given(message)`: records a message the agent was given to read in a turn -
    into the transcript and its history without a context, the turn's context moved on past it,
    `turn_id` set - as `said` records what it says. `Turn::give(message)` for components. A
    message given is not published: it is what the agent read, not what it said.
7.  `SkillSnapshot { id, name, scope, description, classification, can_change }` in
    `snapshot.rs`; the body is read on its own (`SkillView::body`), not with every listing.

### C. Step 2 - the runtime: reading and loading (`caelumora-runtime`)

8.  **`SkillManager`** (`skills.rs`): `create(ctx, name, scope, description, body,
    classification)`, `rename`, `describe`, `rewrite` (the body), `reclassify`, `delete`,
    `restore`, `purge` - stored first, announced after, under the lock the workspace manager has
    for names; the owner is the context's principal, a user or the system. Held by the
    `RuntimeManager` (`skill_manager()`), reached from the views (`Views::skill_manager`,
    `pub(crate)`). `scoped.rs` deletes, restores and purges the skills of a session's scope with
    it.
9.  **The views.** `Views::skills()` and `skill(id)` for a viewer who reads them (`GetSkill`),
    `deleted_skills()` for the trash; `Views::usable_skills()` for an agent reading from a place:
    every skill the viewer may use (`UseSkill`) that is found from the place
    (`Scope::visible_from` with the place's scope - the session's - and the principal as the
    user) and whose classification may be published into the place by the viewer; and
    `usable_skill_named(name)`, `find_by_name` among them - `NoSkillNamed` alike for one that does
    not exist and one that may not be used. `SkillView { snapshot(), body(), may_change() }`.
10. **The component `skills`** (`component_impl/skills.rs`, kind `skills`), parameters `position`
    (`start` by default) and `requires_approval` for the authoring tools (`true` by default):
    - `PromptProvider`: `[Skills you can load]` - what a skill is, that `load_skill` fetches one,
      that a skill a user loaded for the agent stands in the conversation already -, then one line
      per usable skill: name and description. Nothing where there is none. Classified as the join
      of the listed skills' classifications; the part records no knowledge - the catalogue is
      names, the read is recorded when a skill is loaded.
    - `list_skills()`: the catalogue as a tool, for a model whose prompt was built before a skill
      appeared.
    - `load_skill(name)`: `usable_skill_named`, the body rendered under `[Skill: <name>]` as plain
      text, `ToolOutput` classified as the skill, `CALL_ONLY` - the channel sees that a skill was
      loaded, not its text -, the read recorded as `KnowledgeSource::Skill`.
    - `AgentCommandProvider`: `/skill <name>` - `usable_skill_named`; the body as a message of the
      issuer's (`MessageRole::User`, `sender_id` the issuer, `sender_name` as the channel's
      members know them, `classification` the skill's) given to the agent (`Turn::give`), the read
      recorded, answered privately "Loaded skill '<name>'". `/skills` - the catalogue as the
      issuer may see it, answered privately. Neither is `owners_only`: loading a skill for an
      agent is like telling it something, which every member may.
11. **What the catalogue costs.** Every turn's label rises to the join of the usable skills'
    classifications, as it rises with the notes part - never above the channel, since what may
    not be published into it is not listed.

### D. Step 3 - authoring by agents (`caelumora-runtime`)

12. `create_skill(name, description, body, scope)`: `scope` is `session` - the turn's session's
    scope - or `user` - the owner's -; owned by the agent's owner, who must be a user; classified
    as the turn's label (`turn.label()`); through the manager, in the turn's context, so that
    `CreateSkill` is asked for the owner. `update_skill(name, description?, body?)`: the skill as
    found from the place, changed through the manager (`UpdateSkill`: the owner's alone). Both
    require approval unless the component says otherwise; the approver is shown the name and the
    description, and the body whole (`describe`). The model is told the rules in the tools' texts:
    write a skill for what it has learnt to do, not for what it knows (that is memory); keep it
    self-contained; prefer updating to creating twice.

### E. Step 4 - the frontends and the seed

13. **REST** (`routes/skills.rs`): `GET /skills` (`?deleted=true` for the trash), `POST /skills`
    (`{name, scope: user|global, description, body, classification?}`), `GET /skills/{id}` (with
    the body), `PATCH /skills/{id}` (rename), `PUT /skills/{id}/description`, `PUT
    /skills/{id}/body`, `PUT /skills/{id}/classification`, `DELETE /skills/{id}`, `POST
    /skills/{id}/restore`, `POST /skills/{id}/purge`; `SkillResponse`, `Subject::Skill`. A skill
    of a session's scope is made by an agent in it, not here.
14. **Web UI**: a *Skills* section in the sidebar beside *Workspaces*, following `Skill…` events;
    `SkillPage` - name, scope, classification, description, the body in an editor, each changed in
    place -; `NewSkillDialog`. The chat needs nothing new: `/skill` is shown as 269 shows a command,
    the loaded body stands in the agent's history panel as given by the issuer, and the context
    panel lists the skill under "Knowledge read".
15. **TUI**: nothing - commands and the stream are 269's.
16. **Seed**: `skills:` beside `personalities:` and `session_templates:`, skills of the system in
    the global scope; `examples/seed.yaml` gets one, and the assistants get the `skills` component.

### F. Tests

17. Model: `validate` for the scopes; `given` records without a context and moves the turn's
    context on; the policy test for `UseSkill` as reading; the store suite for the kind.
18. Runtime: `usable_skills` leaves out what may not be used and what may not flow into the
    place, finds the innermost of a name; the catalogue lists exactly those; `load_skill` records
    the read and classifies as the skill; `/skill` puts the body into the transcript in the
    issuer's name before the message that follows it, and the channel's history records the
    command, not the body; `create_skill` makes a skill of the owner's in the scope asked for,
    classified as the turn's label, and refuses `global`; `update_skill` refuses another's.
19. Security suite: `an_agent_loads_a_skill_for_its_owner_whoever_asked` (bob's `/skill` loads
    alice's skill into her agent: `UseSkill` asked as the agent for alice, bob the initiator; the
    body is in the agent's transcript, not in the channel's history);
    `a_skill_classified_above_the_channel_is_neither_listed_nor_loaded`;
    `a_skill_is_read_by_its_owner_and_contributors_alone` (REST, views);
    `an_agents_skill_belongs_to_its_owner_and_lies_in_the_session` (`create_skill`).

### G. Documents

20. `docs/SKILLS.md`: what a skill is and is not, where it lives, who may use and write one, how
    it reaches the model - the catalogue, `load_skill`, `/skill` -, the component and its
    parameters, authoring by agents, REST and the web UI; a row in `AGENTS.md`'s table and a
    paragraph in `ARCHITECTURE.md` ("What an agent remembers" gets a sibling: what it can look up).
    `TOOLS-AND-COMMANDS.md`: the tools and the commands in the tables; `COMPONENTS.md`: the
    component; `MEMORY.md`: `KnowledgeSource::Skill` and, under *Terms*, that a skill is no tier
    of memory; `SECURITY.md`: `UseSkill` under the ownership policy; `REST-SERVER.md`,
    `WEB-UI.md`, `CONFIGURATION.md` (the seed's `skills`); `STORAGE.md` if it lists the kinds.

### H. Steps

1.  Step 1 (B): the model, with the store suite and the policy test.
2.  Step 2 (C): manager, views, the component with catalogue, `list_skills`, `load_skill`,
    `/skill`, `/skills`.
3.  Step 3 (D): `create_skill`, `update_skill`.
4.  Step 4 (E): REST, web UI, seed.
5.  The documents (G), then the close.

### I. What to watch

- **`given` writes at once** (issue 260), so `/skill` gives the body only after every check
  passed - a command turn dropped afterwards would leave the body behind.
- **The issuer's name** for the given message: the turn knows the issuer by id
  (`CommandRequest::issuer`); the name comes from the channel's members through the views, "a
  user" if they are gone.
- **A skill loaded twice** stands twice in the transcript; `load_skill` says so in its result when
  the same digest was recorded in this turn already, and loads anyway - the model asked.
- **Compaction (16)** must keep a loaded skill, or load it again; noted there.
- **The catalogue's classification** raises the label; a personality that wants its agents to
  write public notes in a channel with internal skills has to live with it, as with notes.
- **Names**: `Name` rules for entities apply; the model is told them in `create_skill`'s text and
  gets the error otherwise.

## Open questions

1.  **Contributors on a skill**: kept, so that a user shares a skill with a colleague without a
    group scope (issue 23) - as a personality is shared. A workspace has none because it is shared
    through channels; a skill is not.
2.  **Does `/skill` pass the perceive processors?** No: they filter what others say; a skill is
    what a member chose to give the agent, and `load_skill` would pass nothing either.
3.  **32 000 characters** for the body: twice a briefing, enough for a procedure, not for a manual.
    Resources beside a skill - files, examples - are the workspace's, later.
4.  **Versions**: none. What was loaded is in the transcript word for word, with its digest in the
    knowledge; a changed skill is loaded anew next time.
5.  **Preloading** a skill by a personality, and importing `SKILL.md` files, are later.

## Done when

- A `Skill` entity exists with a decided content model — in particular whether a skill may carry
  executable content — stored in the `EntityStore` with `Metadata` and `SecurityAttributes`.
- `ActionType` has separate variants for managing skills and for loading one into a turn, and both
  are checked: the CRUD side in the store/REST path, the loading side in the component that pulls
  the body into the context.
- The skill catalogue offered to the model contains only skills the caller is permitted to load.
- Agents reach skills through the existing `SystemPromptProvider` / `ToolProvider` extension points,
  without a new stage in the cognitive cycle.

## History

- **2026-09-13** — Issue created. Nothing named `skill` exists in the workspace yet, so this is a
  design issue first and an implementation issue second.
- **2026-10-09** — Design discussed with the maintainer; no longer blocked on 12, which 255
  settled (an agent acts for its owner, whoever asked is its initiator). Decided so far:
  - A `Skill` is an entity built as the workspace is (253): `ScopedName` in the global, a user's or
    a session's scope - no channel scope -, a one-line description for the catalogue, a Markdown
    body, one `classification`, `SecurityAttributes`, `deleted`. `GetSkill`, `ListSkills`,
    `CreateSkill`, `UpdateSkill`, `DeleteSkill` as for the workspace, and a `UseSkill` of its own
    for loading one into a turn, beside `CopyInformation` for its classification into the channel.
  - A skill reaches the model only through the agent's own transcript, never through the system
    prompt (the prompt cache, the digest record, branching) and never as a message of the channel's
    history (others would receive text they may not read). Two ways in: the model calls
    `load_skill` and gets the body as a tool result, plain text under a title, `CALL_ONLY`; a user
    types `/skill <name>` and the agent puts the body into its transcript in the user's role, as
    the notices of 236 are put, through the mechanism of [269](269-agent-commands.md), which this
    issue now waits for. The catalogue - name and description of what may be loaded here - is a
    prompt part, `Start` by default.
  - One component `skills`, as `notes` is: the catalogue (`PromptProvider`), `list_skills`,
    `load_skill`, `create_skill`, `update_skill` (`ToolProvider`), `/skills` and `/skill`
    (commands). Reads are recorded as `KnowledgeSource::Skill`.
  - Agents may create skills, within guard rails: owned by the agent's owner, in the session's or
    the owner's scope only, classified as the turn's label (168), with approval by default;
    `update_skill` for the owner's own skills alone. Instructions only, no executable content.
  - Related: 166 wants the same shape for memory (an index in the context, the body by tool); the
    form is shared, the storage is not. A loaded skill must survive compaction (16).
- **2026-10-09** — Decision consolidated and plan written against `2c6dcb3`, now that 269 is
  done; branch `issue-22-skills`.
- **2026-10-09** — Step 1 (B) done: `Skill` (`skill.rs`, `SkillBody` of 32 000 characters), the
  store methods with the memory and JSON stores and the store suite, `GetSkill` … `UseSkill` -
  `UseSkill` granted as reading is -, `GlobalEvent::Skill…` delivered by `GetSkill`,
  `KnowledgeSource::Skill` with `digest::skill`, `SkillSnapshot`, `AgentChannelState::given` and
  `Turn::give`. Beyond the plan: `given` lets the turn see the message, so a skill loaded by
  `/skill` raises the turn's label as one loaded by `load_skill` does; `SkillSnapshot::owner`, for
  the web UI; `KnowledgeRefResponse::skill_id` already, since REST matches every source.
- **2026-10-09** — Step 2 (C) done: `SkillManager` (`skill.rs`, held by the `RuntimeManager`),
  skills deleted, restored and purged with a session's scope (`scoped.rs`), the views
  (`Views::skills`, `skill`, `deleted_skills`, `usable_skills`, `usable_skill_named`,
  `SkillView`), the component `skills` with the catalogue, `list_skills`, `load_skill`, `/skill`
  and `/skills`. Beyond the plan, or decided while building it:
  - **A text result is given as text** (`tool::successful_result`): a tool value that is a JSON
    string reaches the model as the text it is - with the owner's note after it in brackets when
    the channel does not see it -, any other value as JSON. That holds for every tool, so the
    joined text of an `mcp_tool` call is no longer a quoted string either.
  - `scope::found_from` lists what is found from a place, the innermost of every name, beside
    `find_by_name`. The views filter first and choose after: a skill of the session the agent may
    not use does not hide the global one of its name.
  - `Views::cleared_for` moved up from `SessionView`, shared by channels and skills: a user is
    shown a skill only if cleared for its classification, as a channel. The manager asks
    `Classify` and the owner's clearance when a skill is made or reclassified, as for a
    workspace, so that nobody writes what they could not read.
  - `AccessQuestions::may_change_skill` and `may_use_skill`.
  - `/skills @agent` lists what the agent may load for its owner, not what the issuer may read:
    the agent answers for its owner, and whoever may ask it may ask what it knows (issue 255).
    The names and descriptions of the owner's skills reach the issuer that way, privately - the
    consequence of the owner putting the agent into the channel, not a gap in the rule.
  - The parameter `requires_approval` comes with the authoring tools in step 3.
