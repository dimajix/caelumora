# 323 — Every stored kind is enumerated in the core, so no subject can leave it

**Status:** in progress
**Area:** Code structure

## The problem

The maintainer wants to move single subjects - skills first, other components of agents later -
into crates of their own, so that the core stays small and a subject is added or left out by
linking it. The `ComponentRegistry` already allows that for components: a kind is registered with
its factory and facets, and nothing in the core names it. But a subject such as skills is more
than a component. It is a stored entity with permissions, events, a place in a scope, views, a
seed and a REST resource, and it is known to a few features of the core besides - and each of
these the core enumerates in a closed `enum` or a fixed field, which a crate outside the core
cannot extend. For skills these are:

| Where | How skills are wired in |
|---|---|
| `ActionType` (`caelumora-model/src/security.rs`) | six variants, each repeated in `describe()` |
| `OwnershipSecurityManager::check` | the six variants in its match arms |
| `AccessQuestions` | `may_change_skill`, `may_use_skill` |
| `EntityKind` (`caelumora-model/src/store/mod.rs`) | `Skill`, keyed by `ReadDeleted`, `Restore`, `Purge`, `ScopeNotEmpty` |
| `EntityStore` | ten `…_skill` methods, a field per store, the `SKILLS` table in `store::access` |
| `GlobalEvent` | `SkillCreated`, `SkillUpdated`, `SkillDeleted`, `SkillRestored`; `PublishedEvent::read_action` maps them to `GetSkill` |
| `Scoped` (`caelumora-runtime/src/scoped.rs`) | a `skills: Arc<SkillManager>` field, called in a fixed order |
| `Views` | `SkillView`, `skills()`, `usable_skills()`, `skill(id)`, `ViewError::SkillNotFound`, `NoSkillNamed` |
| `Seed` | `skills: Vec<Skill>` |
| `RuntimeManager` | `skill_manager()` |
| `KnowledgeSource` (`caelumora-model/src/context.rs`) | `Skill { skill, digest }`, recorded in transcripts; `digest::skill` |
| `SummaryCompaction` | gives the skills of summarised messages again (`skills_read`, `CompactionError::SkillsUnreadable`, `Compaction::skills_given`) |
| `ToolPresentation` (`caelumora-model/src/tool.rs`) | `Skill`, `SkillList` |
| `caelumora-model` besides | `skill.rs`, `SkillBody` in `value::text`, `SkillSnapshot` in `snapshot` |
| `caelumora-rest-server` | the routes, the wire types in `models.rs`, `Subject::Skill`, `ApiError::from_runtime` downcasting to `SkillError` |
| `security_suite.rs` | the skill tests, the names of the actions it logs, and a `World` fixture no other crate can use |

The same holds for workspaces, artifacts and groups. Only components and tools are open today;
everything a subject stores is closed, and three features of the core - the transcript, the
compaction, the presentation of a tool call - have a case of their own for skills.

## Why it matters

- A subject cannot be moved out of the core, nor added without touching a dozen places of it. The
  list above is the checklist every new entity kind goes through by hand today (issues 22, 251,
  271, 286 each did), which is the same cost in the other direction.
- Much of what the core enumerates is repetition: the ownership policy decides every `Get*` alike,
  every `Create*` alike, every `Update*` and `Delete*` alike, and `describe()`, the `Actions`
  tables and `read_action` say per kind what is the same for all. Opening the core removes that
  repetition whether or not a plugin ever uses it.
- Where the core stays closed, a plugin ends up with a shadow of its own - its own permission
  check beside the policy, its own event beside `GlobalEvent` - which is exactly the "second
  implementation" `AGENTS.md` warns of.

## Decisions taken

Agreed with the maintainer while this issue was written; the reasoning is under *Considerations*.

- Plugins are crates linked into the binary; no loading at run time.
- Skills are the one subject moved out in this issue. Workspaces, artifacts and groups follow in
  issues of their own, once the seams have shown that they hold.
- The web UI stays one application and is changed along wherever the API changes - the event
  names first.
- The REST side is cut in three: a wire crate `caelumora-rest-api`, the server as a library
  `caelumora-rest-server`, and the application `caelumora-app`, which lists the plugins, serves
  the API under `/api` and ships the web UI embedded in the binary.
- A plugin is one crate, layered inside by Cargo features, not three crates.

## Considerations

The first question is *how* a plugin is joined to the core at all; the rest is what the core has
to offer for that to be possible - the cuts, one by one.

### How plugins are joined

- **Statically, as crates linked into the binary.** A plugin implements the core's traits and
  registers itself when the runtime is built; which plugins are in is decided by the binary's
  dependencies or Cargo features. Everything the facets need - `async`, `Arc<dyn …>`, the
  `SecurityContext`, the stores - crosses the boundary as it is. This is the way the
  `ComponentRegistry` already works and the only one that needs no second interface.
- **Dynamically loaded libraries** (`cdylib`, `libloading`) are rejected for Caelumora: Rust has no
  stable ABI, so nothing of the facets' shape - trait objects, `String`, futures - crosses the
  boundary without `abi_stable` or an `extern "C"` layer, a plugin has the process's full rights,
  and panics and `tracing` subscribers do not cross cleanly either.
- **WebAssembly or a separate process** would let a plugin be added without recompiling and
  sandbox it. Both need a serialisable interface designed per facet, with the security context,
  the classifications and the stores modelled as host functions or RPC. That is a second interface
  beside the traits, and nothing asks for it yet. `mcp_tool` is already this shape for the one
  facet where an external process is the point. Left for a later issue, if ever; the work below
  prepares it in any case, since what is open for a crate is open for an adapter too.

**Decided:** static plugins. A `Plugin` trait bundles what a plugin contributes, after the pattern
of Bevy's `Plugin::build(&mut App)`:

```rust
pub trait Plugin {
    fn register(&self, runtime: &mut RuntimeBuilder);
}
```

`RuntimeBuilder` is where today's constructor of the `RuntimeManager` takes the registry, the
defaults and the stores. Through it a plugin registers its component kinds (as today), its entity
kind, its managers as members of a scope, its seed and configuration sections and its views. A
second trait, `RestPlugin`, does the same for routes on the REST side. `Plugin` is the one new
type with a responsibility of its own - composition - and the binaries become composition roots
that list the plugins.

### Cut 1 - `ActionType`: resource and verb instead of a variant per pair

The policy already decides by the verb, not by the kind: `OwnershipSecurityManager::check` folds
every `Get*` into `granted(Read)`, every `Create*` into "the caller is the owner", every `Update*`
and `Delete*` into "the stored owner", every `List*` into `true`. The variants are the product of
kind and verb, written out, and the policy folds it back.

- **Option A - keep the closed enum; a plugin adds its variants to the core.** No change, no
  plugin: a subject that wants a verb of its own edits the core's `ActionType`, its `describe()`,
  every policy and the suite. The repetition stays. Rejected: this is the status quo, and the
  thing to get rid of.
- **Option B - `Entity { kind, verb, attributes }`.** One variant for what every stored kind is
  asked alike, as Kubernetes and IAM name a permission by resource and verb:

  ```rust
  pub enum ActionType<'a> {
      Entity { kind: EntityKind, verb: Verb, attributes: &'a SecurityAttributes },
      List(EntityKind),
      ApproveToolCall { .. }, ModifyMessage { .. }, Reclassify(..), ShowCommand { .. }, ...
  }
  pub enum Verb { Get, Create, Update, Delete, ReadDeleted, Restore, Purge, Custom(&'static CustomVerb) }
  pub struct CustomVerb { pub name: &'static str, pub requires: Requirement }
  pub enum Requirement { Grant(Grant), Owner }
  ```

  `UseSkill` becomes `Custom(&USE)` with `requires: Grant(Read)`; `WriteWorkspace` and
  `WriteArtifact` become `Custom(&WRITE)` with `Grant(Write)` - two special cases of today become
  one rule. The actions that are no act on one stored entity - approving, modifying a message,
  reclassifying, the commands, `CallTool`, `ReadSecret`, `ReadAgentContext`, `ReadAgentTools`,
  `ReadAgentComponents`, `StopSession`, `CancelChannel`, `WakeChannel`, `RetryMessage`,
  `AnswerQuestion` - stay variants of the core: they are its own, and a plugin does not bring
  such. `describe()` becomes `format!("{verb} {kind}")`.

  *Benefit:* `describe()`, the `Actions` tables of `store::access`, four of the policy's five big
  match arms and the suite's action names collapse into one rule each; a new kind brings no change
  to the core. *Cost:* the policy loses exhaustiveness over kinds and custom verbs, which is a
  security property - today a policy that forgets a new action does not compile. Kept by making
  the policy decide a `Custom` verb by its `Requirement`, which stays a closed enum: a plugin can
  map a verb onto a right the policy knows, never invent a right. A policy that wants to treat one
  verb of one kind differently matches `(kind, name)` besides, as it may today. The
  `SecurityManager`'s contract in `docs/SECURITY.md` has to say this. A `Custom` verb's `name` is
  unique per kind, which the registration checks.
- **Option C - a `Verb` per kind, defined by the plugin.** A trait `Securable` on the entity type
  with associated verbs. More types, and the policy would decide by a trait object instead of a
  value it can match on. Rejected for B: B gives the policy a value to match and keeps the verbs a
  plain enumeration.

**Recommendation:** B. It is the largest step and touches every check, the suite and the two
security documents, and is worth doing first and alone: a refactoring without a behaviour change.

### Cut 2 - `EntityKind`: open, but not free

- **Option A - `EntityKind(&'static str)`** with constants per subject (`pub const SKILL:
  EntityKind = EntityKind("skill")`). Open, cheap, and the name is already what `name()` returns
  and what the JSON store keys its files by - stored data is untouched. *Cost:* two crates could
  claim the same name; the `RuntimeBuilder` refuses a second registration of a name. Matching on a
  kind in the core becomes string comparison, but after cut 1 the core has nothing left to match
  on.
- **Option B - a registered id handed out by the builder.** Avoids the clash by construction but
  makes a kind a runtime value a constant cannot name, which the `Actions` tables, the seed and the
  errors want.

**Recommendation:** A.

### Cut 3 - the rules in the `SecurityManager`

After cut 1 no kind-specific rule is left in `OwnershipSecurityManager`: it decides by the verb's
requirement. `may_change_skill` and `may_use_skill` of `AccessQuestions` move into the plugin as
an extension trait `SkillAccess` with the same blanket implementation over every
`SecurityManager` - the pattern `AccessQuestions` itself uses, in another crate, which the orphan
rule allows because the trait is the plugin's. *Cost:* none in code; the rule "a pure access
question is a method of `AccessQuestions`" in `AGENTS.md` becomes "…or of a plugin's extension of
it". A deployment's policy that wants to decide skills differently still has to know the skill
kind; that is the policy's business, not the core's.

### Cut 4 - the `EntityStore`

The store trait has ten methods per kind and a field per kind in every implementation, though
`MemoryStore` and `JsonFileStore` implement each by one generic `…_in` per operation over an
`Actions` table.

- **Option A - a generic `EntityStore<T>` per kind, with the implementation generic over `T:
  Stored + Scoped + …`.** The plugin asks the runtime for `Arc<dyn EntityStore<Skill>>`; the store
  implementations become one generic type each, instantiated per registered kind. What is per kind
  today - the `find_…_by_name`, the deleted - is what the traits `Scoped` and `Stored` already
  describe. The JSON store needs the kind's name for its files and the collection to be registered
  before it is read; the `RuntimeBuilder` opens the collections, the stores are made per kind from
  one factory that holds the shared parts (the directory, the security manager, the policy, the
  memberships).
- **Option B - keep one trait with every kind and let a plugin bring a store of its own.** A second
  store implementation per plugin, with the access checks of `store::access` written again or
  exposed. Rejected: the checks are the point of the store, and they would be copied.

**Recommendation:** A. *Cost:* the largest mechanical change of the issue (the two store
implementations, the store suite in `store::suite`, every `entities.get_skill(…)` becomes
`skills.get(…)`). Where it pays: every kind added since issue 93 wrote the ten methods and the
table by hand. *Open:* whether the kinds the core keeps (sessions, channels, agents, users, …)
move to the generic store in the same step or stay on the big trait until a later issue; the
generic store has to exist for skills either way, and the big trait can be implemented over it.

### Cut 5 - `GlobalEvent`

The lifecycle events are four per kind, and `PublishedEvent::read_action` maps each four to the
kind's `Get`.

- **Option A - one variant `Entity { kind, id, change }`** with `change` an enum `Created |
  Updated(Value) | Deleted | Restored`, and `read_action` mapping it to `Entity { kind, verb:
  Get }` by one rule. The payload of an update (`SkillUpdateEvent`) is the plugin's type; it goes
  as `serde_json::Value`, which the plugin writes and reads. *Cost:* the JSON form of the events
  changes, and `caelumora-web-ui` reads it (`Sidebar.tsx` matches `SkillDeleted`,
  `WorkspaceRestored`, …).
- **Option B - the plugin defines its events as a type the bus carries boxed.** The bus would
  carry `Box<dyn Event>`; the web UI would get what each plugin serialises. More open, but the bus
  could no longer say what a party must be allowed to receive an event without asking the plugin,
  and the event streams of the REST server would become untyped. Rejected until a plugin has an
  event that is no lifecycle event.

**Decided:** A, with the web UI changed along; the generic form is simpler to read there too.

### Cut 6 - what lies in a scope

`Scoped::delete_lying_in` calls each manager by name, in a fixed order, and gathers the counts
by `EntityKind`. The order matters: groups go last, and only when nothing is left.

A trait `ScopeMember` (`delete_lying_in`, `restore_lying_in`, `purge_lying_in`), implemented by
each manager and registered by the plugin, kept by `Scoped` as a list. The order becomes the
registration order; the group rule stays a rule of `Scoped`, not of a member, since it is about
the scope as a whole. *Cost:* one trait; the counts come out as `(EntityKind, usize)` as today.

### Cut 7 - what the core's features know of skills

Three features of the core have a case for skills that is no enumeration of kinds but a feature
of their own. They are the hidden couplings, found while writing this issue, and the most likely
to be forgotten.

- **`KnowledgeSource::Skill { skill, digest }`** records in a transcript that a skill was given.
  Opened as a variant `Entity { kind: EntityKind, id: Uuid, digest: Digest }`, which serves a
  workspace's file as well. *Cost:* the serialised form of transcripts changes: the agent channel
  states in the state store have to be deleted. `digest::skill` moves into the plugin; the
  `Encoder` it uses becomes public.
- **`SummaryCompaction` gives skills again** that were loaded in the summarised part and are not
  loaded in the kept part: it reads `KnowledgeSource::Skill` out of the transcript, loads the
  skills through `Views::usable_skills` and writes them as messages after the summary. Options:
  - *keep it in the core, written against the skill kind:* the core would depend on the plugin,
    which is the one direction that is forbidden;
  - *a registry of "knowledge givers" per `EntityKind`:* a new registry, kept by the runtime,
    asked by the compaction - a second mechanism beside the components;
  - *a facet of the component that gave the knowledge* (recommended): a trait
    `KnowledgeRestorer` beside the other facets, which the compaction asks of every component of
    the agent: "of the knowledge recorded as given in these entries, which do you give again, and
    as what message?". The skills component, which loads skills into the turn, implements it; the
    compaction knows nothing of skills and works for whatever a later plugin gives. This is the
    existing extension point - a component facet - used for a new question, as `PromptProvider`
    and `ContextProvider` are.
  `Compaction::skills_given` and the sentence `/compact` answers with it become generic
  (`given_again`, by name). `CompactionError::SkillsUnreadable` becomes a general "knowledge
  unreadable".
- **`ToolPresentation::Skill` and `SkillList`** say how the web UI renders a tool call. The
  vocabulary is the UI's contract, and the UI is one application that knows every presentation,
  so the list is closed *there* by nature. Still the core model names skills; opened as a newtype
  over a string (`ToolPresentation("skill")`), its serialised form unchanged, so that a plugin
  names its presentation and the UI renders what it knows. *Cost:* small; exhaustiveness in the
  UI's `switch` is TypeScript's business already.

Besides these, what is simply the plugin's moves without a cut: `caelumora-model/src/skill.rs`,
`SkillBody`, `SkillSnapshot`, `SkillManager`, `SkillView`, the component, the routes.

### Cut 8 - views, seed, configuration, errors

- **Views:** a plugin's view type lives in the plugin; `Views` offers what every view needs - the
  context, the security manager, the stores by kind, `channel_reading` - and the plugin builds its
  own from it, as an extension trait `SkillViews` on `Views` (`skills()`, `usable_skills()`,
  `skill(id)`). `ViewError` loses its skill variants; the plugin's errors are its own.
- **Seed:** `Seed` keeps the core's sections and a map of the plugins' by name, each a
  `serde_json::Value` the plugin deserialises when it registers; a section nobody claims is an
  error after registration. `AppConfig` the same way, for a plugin that has settings.
- **REST errors:** `ApiError::from_runtime` downcasts to every error type of the runtime. A plugin
  implements `From<SkillError> for ApiError` itself - the orphan rule allows it, the type is the
  plugin's. `Subject::Skill` goes: with `From` per error type the subject is known where the error
  is made.

### Cut 9 - the REST side, in three layers

Issue [307](307-load-and-performance-tests.md) wants the wire types out of the server so that a
client crate can read them. That and this issue's need - a public API for a plugin's handlers -
lie on different axes; together they give three layers with one direction of dependency:

```
caelumora-model
      ▲
caelumora-rest-api              the wire: request and response types, serde, nothing else
      ▲          ▲
      │     caelumora-rest-client   reqwest, login, streams  ◄── the load test (307), later the TUI
caelumora-rest-server (lib)     AuthenticatedUser, ApiError, Json/Path, views(), SSE, the core's routes
      ▲
caelumora-skills                its routes, From<SkillError> for ApiError
      ▲
caelumora-app                   the plugins, /api, the web UI, main
```

- **The wire** depends on the model alone. `models.rs` today also implements `From<SessionView>`
  and `From<Reached<…>>` (`caelumora_runtime::view`); those conversions move into the server
  library, where the views are known. Its own crate rather than the client crate, as 307 had it:
  the server would otherwise depend on the client, which reads wrong and would pull `reqwest`
  into the server unless feature-gated. `caelumora-client-api` is not this: it is reserved for a
  transport-independent client interface and stays as it is.
- **The server library** is everything a handler needs - `AuthenticatedUser`, `ApiError`, the
  `Json` and `Path` extractors, `views()`, the SSE helpers - made public, and the core's routes as
  a `Router` to merge. A handler reaches its manager through its own state, built when the router
  is built, not through `state.runtime_manager.skill_manager()`.
- **The application** `caelumora-app` is the composition root: it lists the plugins, builds the
  runtime through the `RuntimeBuilder`, nests the API under `/api` and serves the web UI at `/`.
  The URLs of the API stay as they are below `/api`, which is what the web UI already speaks
  (`API_BASE = '/api'`; the Vite dev proxy rewrote it away so far and stops doing so, or keeps it
  for running against the API alone). `caelumora-tui` stays a composition root of its own, since
  it speaks to the runtime directly.

### Cut 10 - the web UI in the binary

Decided: `caelumora-app` ships the web UI.

- **Embedding** with `rust-embed` on `../caelumora-web-ui/dist`, served by `axum`, behind a Cargo
  feature `web-ui` (default on): one binary, nothing beside it to deploy; without the feature a
  pure API server, which the load test and development with `vite dev` want. `rust-embed` reads
  from disk in debug builds, so a change of the UI needs no Rust rebuild there. `ServeDir` from a
  configured directory would keep the UI replaceable apart from the binary, at the price of two
  artifacts; not wanted.
- **The build-time dependency.** Cargo cannot depend on an npm package; a `build.rs` in
  `caelumora-app` is the way. It does *not* run `npm`: that would make Node a requirement of every
  `cargo build`, `cargo test` and the pre-commit hook's `cargo clippy --workspace --all-targets`,
  and slow every clean build. It checks that `dist/index.html` exists and fails with a message
  that says to run `npm run build` in `caelumora-web-ui`, with `rerun-if-changed` on `dist` so
  that a new UI build is embedded; running npm from the build can be switched on by an
  environment variable (`CAELUMORA_BUILD_UI=1`) for release builds. Node is a requirement of the
  release binary, not of every Rust build.
- **Serving:** `index.html` with `Cache-Control: no-cache` and as the fallback for paths that are
  no file and not under `/api`, so that a client-side route works should the UI get one; the
  hashed files under `dist/assets` cacheable for long.

### Cut 11 - a plugin crate and its layers

`ARCHITECTURE.md` cuts the core by layer - model, runtime, REST - and a plugin holds all three
layers of one subject plus its wire types, which the client must read without pulling the runtime
and `axum`. Options for where a plugin's wire types live:

| | Benefit | Cost |
|---|---|---|
| **A** in the plugin crate, no further structure | one crate | a client pulls the runtime and `axum`; no layer enforced |
| **B** a crate of its own, `caelumora-skills-api` | mirrors the core's cut | two crates per plugin |
| **C** in the plugin crate, behind features `runtime` and `server`; a client depends with `default-features = false` | one crate, the layers enforced by features | less visible than a crate; the conversions from the runtime's types live behind the feature |

**Decided:** C. The core is cut into crates because it is large, a plugin into features because
it is small: a deliberate difference, with this reason. Without features `caelumora-skills` is
the entity (`Skill`, `SkillId`, `SkillBody`, `SkillSnapshot`) and the wire types; `runtime` adds
the manager, the component, the view extension, the scope member, `SkillAccess`; `server` adds
the routes and `From<SkillError> for ApiError`; `client` adds typed calls as an extension trait
`SkillsClient` on the REST client - the same pattern as the server-side extensions. The default
is all of them.

### What is not decided here

- Which subjects follow skills, and whether some stay in the core because everything depends on
  them (sessions, channels, agents, users certainly do). Each is an issue of its own.
- Loading plugins without recompiling (WebAssembly, a process): out of scope, see above.
- Whether the core's own kinds move to the generic store in cut 4 or later.

## Implementation plan

After *restructure, then extend*: steps 1 to 8 open the core without a plugin, each a change of
its own on its own branch without a behaviour change, reviewable alone; step 9 moves skills out,
which shows whether the seams hold. Every step runs the tests and `cargo clippy --workspace
--all-targets`, updates the documents it names, and says which stored data has to be deleted.

### Step 1 - `ActionType` by resource and verb (cuts 1-3, in part)

- `ActionType::Entity { kind, verb, attributes }` and `List(kind)`; `Verb`, `CustomVerb`,
  `Requirement`; `describe()` by one rule. The constants `USE` and `WRITE` for the custom verbs,
  in the model for now, next to the kinds they belong to.
- `OwnershipSecurityManager::check` decides `Entity` by the verb's requirement; the special cases
  that stay (`Purge` the system's) stay. `DummySecurityManager` unchanged.
- `store::access`: the `Actions` tables become one `Actions::of(kind)`.
- `AccessQuestions` keeps its skill methods for now - they move in step 9 - and builds the new
  actions.
- `caelumora-runtime/src/event.rs`: `read_action` builds `Entity { verb: Get }`.
- `security_suite.rs`: the actions it logs by name (`UseSkill(alice's)`) are now written as
  `describe()` writes them; the assertions follow.
- Documents: `SECURITY.md` (the policy decides a custom verb by its requirement; a plugin maps,
  never invents), `PERMISSION-CHECKS.md` (the names of the actions), the doc comments of
  `ActionType`.
- Stored data: none.

### Step 2 - `EntityKind` open (cut 2)

- `EntityKind(&'static str)`, constants per kind where the entity type lives, `name()` returns the
  string, `Display` as before. Small; may be folded into step 1 if that reads better.
- Stored data: none - the names are those of today.

### Step 3 - the generic `EntityStore<T>` (cut 4)

- `trait EntityStore<T: Stored + Scoped>` with `create`, `get`, `update`, `delete`, `list`,
  `restore`, `purge`, `get_deleted`, `list_deleted`, `find_by_name` - the ten of today, once.
- `MemoryStore<T>` and `JsonFileStore<T>` as generic types made by a `StoreFactory` that holds
  what they share; the big trait, for the core's kinds, implemented over them until the follow-up
  issues move the kinds (decided in this step).
- `store::suite` generic over `T`, run for every kind as today.
- The callers of the skill methods (`SkillManager`, `Seed`, `Scoped`, `Views`, the suite) take
  `Arc<dyn EntityStore<Skill>>`.
- Documents: `STORAGE.md`.
- Stored data: none, if the JSON store keeps its file layout per kind name; say so in the summary
  if it does not.

### Step 4 - `ScopeMember` (cut 6)

- The trait, implemented by `AgentManager`, `WorkspaceManager`, `SkillManager`,
  `ArtifactManager`, `GroupManager` and, for personalities and session templates, the small
  inline loops of today; `Scoped` holds `Vec<Arc<dyn ScopeMember>>` in registration order and
  keeps the group rule.
- Documents: `CHANNELS.md` where it describes the deletion of a scope (issue 320).
- Stored data: none.

### Step 5 - `GlobalEvent::Entity` and the web UI (cut 5)

- `GlobalEvent::Entity { kind, id, change }` replaces the lifecycle variants of workspaces,
  skills, artifacts and groups - of every kind that moves or may move; the core's own may follow
  or stay, decided in this step by what reads simpler in the UI. `read_action` by one rule.
- The REST event streams (`sse.rs`) carry the new form; `caelumora-web-ui` reads it
  (`Sidebar.tsx`, the event handling of the lists).
- Documents: `EVENTS.md`, `REST-SERVER.md` (the event's JSON), `WEB-UI.md`.
- Stored data: none - events are not stored.

### Step 6 - knowledge, compaction and presentation opened (cut 7)

- `KnowledgeSource::Entity { kind, id, digest }` replaces `Skill` (and `Workspace`, which is the
  same shape with an optional digest - decided in this step).
- `KnowledgeRestorer` as a facet in `crate::compaction` (where it is used, by the rule of
  `COMPONENTS.md`); the skills component implements it; `SummaryCompaction` asks the container's
  restorers and knows no kind; `Compaction::given_again`, the `/compact` answer and
  `CompactionError` generic.
- `ToolPresentation` as a newtype over a string, serialised as today; constants where the tools
  live.
- Documents: `AGENT-TURN.md` (the transcript's knowledge), `COMPONENTS.md` (the facet), the
  compaction's doc comments.
- Stored data: **the state store** - agent channel states hold transcripts with
  `KnowledgeSource` - has to be deleted.

### Step 7 - the wire crate (cut 9, first half; the preparation of 307)

- `caelumora-rest-api`: the request and response types of `models.rs`, with `serde` and the
  model's ids and values; no `axum`, no runtime.
- The conversions from `SessionView`, `Reached` and the snapshots move into the server as
  functions or `From` impls on the server side; `models.rs` goes.
- 307 is pointed at this step for its client.
- Documents: `ARCHITECTURE.md` (the crates), `REST-SERVER.md`.
- Stored data: none.

### Step 8 - the server library, `caelumora-app`, the embedded UI (cuts 9, 10)

- `caelumora-rest-server` becomes a library: `auth`, `error`, the extractors, `state`, `sse` and
  `routes::core()` public; `ApiError::from_runtime`'s downcasts become `From<…> for ApiError`
  per error type, each next to its error's crate boundary; `Subject` goes or shrinks to what the
  core's own handlers need.
- `caelumora-app`: `main.rs` from the server, the router nested under `/api`, `rust-embed` behind
  `web-ui`, `build.rs` as in cut 10, the fallback to `index.html`, the cache headers.
- `caelumora-web-ui`: the Vite proxy no longer rewrites `/api`.
- Documents: `ARCHITECTURE.md` (crates, composition root), `REST-SERVER.md` (`/api`, serving the
  UI, how a plugin adds routes), `WEB-UI.md` (building for the binary), `README.md` (how to build
  and run).
- Stored data: none. Deployments change their start command and base URL.

### Step 9 - `Plugin`, `RuntimeBuilder`, the plugins' sections (the composition)

- `Plugin` and `RestPlugin`; `RuntimeBuilder` takes what `RuntimeManager::new` takes, opens
  store collections per registered kind, keeps the `ComponentRegistry`, the scope members, the
  seed and configuration sections, and builds the `RuntimeManager`; `caelumora-app` and
  `caelumora-tui` build through it with an empty list of plugins.
- `Seed` and `AppConfig` with a map of plugin sections; a section nobody claims is an error.
- `Views` opens what a plugin's views need (the context, the security manager, the stores by
  kind, `channel_reading`).
- The `World` fixture of `security_suite.rs` becomes reusable: a `test-support` feature of
  `caelumora-runtime` exposing it, or a crate `caelumora-testing`; decided in this step.
- Documents: `ARCHITECTURE.md` (how a plugin is joined), `COMPONENTS.md` (`Plugin` beside the
  registry), `CONFIGURATION.md` (plugin sections), `AGENTS.md` (the rule on `AccessQuestions`
  and the extension traits; a plugin's tests in the suite).
- Stored data: none.

### Step 10 - `caelumora-skills`

- The crate with the features of cut 11; moved into it: `skill.rs`, `SkillBody`, `SkillSnapshot`,
  `digest::skill`, `SkillManager`, `SkillView` as `SkillViews` on `Views`, the component
  (`component_impl/skills.rs`, with its `KnowledgeRestorer`), `SkillAccess`, the `ScopeMember`,
  the seed section (`skills:` of the seed files, which keep their form), the routes, the wire
  types, `From<SkillError> for ApiError`, `SkillsClient`, the skill tests of the security suite
  and of the compaction.
- `caelumora-app` and `caelumora-tui` list it; the core compiles and its tests pass without it.
- Documents: `SKILLS.md` (where the code lives), `ARCHITECTURE.md` (the first plugin named),
  `LLM-MODELS.md` if it names the component.
- Stored data: none beyond step 6.

### Step 11 - the follow-ups

- Issues for workspaces, artifacts and groups as plugins, each citing what step 10 found.
- 307 takes up `caelumora-rest-api` and `caelumora-rest-client`.

## Done when

- A stored kind is registered, not enumerated: the core's `ActionType`, `EntityKind`,
  `EntityStore`, `GlobalEvent`, `Scoped`, `Seed`, `KnowledgeSource`, `ToolPresentation`,
  `ApiError` and routes name no kind that is not the core's own, and the compaction gives
  knowledge again without knowing whose.
- `caelumora-skills` is a crate of its own, linked by the binaries, with the `skills` component,
  the `SkillManager`, its views, its routes, its wire types and its tests; the core compiles and
  passes its tests without it.
- `caelumora-app` serves the API under `/api` and the web UI from the binary; the web UI works
  against it with the event changes of step 5 made along.
- `docs/ARCHITECTURE.md` says how a plugin is joined and what the core offers; `SECURITY.md` says
  how a policy decides a plugin's verb; `COMPONENTS.md` names `Plugin` beside the registry;
  `REST-SERVER.md` says how a plugin adds routes and how the UI is served.

## History

**2026-10-12** — Opened after discussing how plugins are done in Rust: statically linked crates,
dynamic libraries, WebAssembly, processes and embedded scripts. The maintainer named the four
hard-wired places (the entity store, `ActionType`, the policy, the REST routes) and asked how to
address them for a real modularisation; the list grew to sixteen, among them three couplings of
the core's own features (the transcript's `KnowledgeSource`, the compaction, `ToolPresentation`).
Decided the same day: static plugins; skills alone in this issue; the web UI changed along; the
REST side in three layers with `caelumora-app` as the application, which embeds the web UI; one
crate per plugin, layered by features. The plan above has eleven steps.

**2026-10-12** — Step 1 done, on branch `issue-323-action-by-verb`: `ActionType::Entity { kind,
verb, attributes }` and `List(kind)` replace the variants per kind and verb, with shortcuts per
verb (`ActionType::get(kind, attributes)`, …, `custom(kind, &verb, attributes)`) asked for by the
maintainer. `UseSkill` is `skill::USE`, `WriteWorkspace` and `WriteArtifact` are `WRITE`, each a
`CustomVerb` with its `Requirement`. `describe()` names an act on an entity by verb and kind as
before (`GetSession`, `UseSkill`), a listing as `List(session)`; the security suite now names the
kind in `ReadDeleted`, `Restore` and `Purge` as well. The `Actions` tables of `store::access` are
gone: the stores pass the `EntityKind`. `EntityKind` is still a closed enum (step 2), and
`AccessQuestions` keeps `may_change_skill` and `may_use_skill` until step 10.

**2026-10-12** — Step 2 done, on branch `issue-323-open-entity-kind`: `EntityKind` is a struct over
its name, and every stored type names its kind as `Stored::KIND`, decided with the maintainer over
an inherent constant or one per module, since the generic store of step 3 needs the kind from the
type. The store's helpers take it from `T` and lost their `kind` parameter, so a kind can no longer
be passed to the map of another; whoever names a kind writes `Skill::KIND`. Nothing stored changed:
the names are those of before.

**2026-10-12** — Step 3 decided with the maintainer: every kind moves to the generic store in this
step (option A), by way of the old trait as a facade over the stores of the kinds (option B), kind
by kind in commits of their own, the facade deleted at the end. The runtime is handed a struct with
a field per kind of the core, a plugin makes its store through a factory; the JSON store's
directory of a kind is named after the kind. The `ComponentStore` was to become an
`EntityStore<StoredComponents>` as well; it stays a trait of its own, since a component is removed
when deleted, and a permanent delete in the generic trait would let any kind be removed past its
trash, with `Delete` alone instead of `Purge`.

Step 3a, on branch `issue-323-generic-store`: `Stored` says the id, the query fields and the unique
name of its kind (`unique_name`, from `Scoped`), so that a store needs nothing else of a kind; the
`impl_deletable!` macro is gone. `EntityStore<T>` is the generic trait, implemented by
`MemoryEntityStore<T>` and `JsonEntityStore<T>`; the old trait is `Entities` for the transition,
implemented by `MemoryStore` and `JsonFileStore` over one store per kind. The JSON store keeps a
kind's documents in `entities/<kind>/`: stored entities move there (`templates/agents` to
`entities/personality`, `templates/sessions` to `entities/session_template`, `templates/components`
to `entities/components`, `runtime/<kind>s` to `entities/<kind>`), or are deleted.

Step 3b: `EntityStores` is what the runtime is given - a store per kind, beside `entities`, the
facade, while kinds still use it -, made by the configuration (`create_entity_stores`) or by
`MemoryStore::entity_stores` and `JsonFileStore::entity_stores`. Skills are the first kind on their
own store: the `SkillManager`, the views, the seed and the tests take `EntityStores::skills`, and
`Entities` has no skill methods any more. The store suite takes `EntityStores`, so that each kind
that follows changes only the field it uses. Paths written out in the lines this step touched are
imports.

Step 3c: workspaces on their own store. The `WorkspaceManager` keeps it beside the facade, which it
still reads sessions, channels and groups through, and finds a workspace by name for a template
(`find_by_name`), so that `template::resolve_workspace` no longer reaches into the store. A manager
is made with `&EntityStores` and takes the stores it uses - the `WorkspaceManager` and the
`SkillManager` alike -, so that its constructor does not grow with every kind that moves.

Step 3d: artifacts on their own store. The `ArtifactManager` keeps it beside the facade, which it
still reads groups through; the views reach the stores of the kinds as `Views::stores`.

Step 3e: groups on their own store. Groups are read and written beyond their manager: the
`GroupManager`, the access lists the workspace and artifact managers are given
(`require_nameable`), the groups of participants that sessions, channels and the
`ChannelController` keep (`keep_participants`, `purge_participants`), and the policy's
`StoreMemberships`, now bound to the store of groups. The free functions take that store; the
`SessionLoop`, the `SessionManager` and the `ControllerManager` hold `EntityStores`, as the views
do, since they use many kinds; the `ArtifactManager` no longer needs the facade.

Step 3f: session templates on their own store. They have no manager: `ScopedEntities` keeps the
store, made from `&EntityStores` as the managers are, the `SessionManager`, the seed, the REST
route of templates and the terminal frontend take it from `EntityStores`.

Step 3g: user profiles on their own store. The `UserController` keeps it - the only store it used
-, the member view of a user and the preferred language of a new session or channel read it, and
the REST route of the profile takes it from `EntityStores`.

Step 3h: personalities on their own store. They have no manager either: `ScopedEntities` keeps the
store and no longer the facade, the `AgentController` and the member view of an agent read an
agent's personality from it, and `template::resolve_participants` takes `&EntityStores` to read
the personality an agent is made of. Many tests made personalities through the store they had at
hand; they take the store of personalities now.

Step 3i: users on their own store. The `UserManager` and the `accept_any` provider keep it, given
by the configuration (`create_user_manager`); a provider finds a user by name as the name in the
global scope, which a username is (issue 250), and one that is no valid name is nobody's. The
`GroupManager` checks the users it is given against it and no longer needs the facade; the views,
the templates and the terminal frontend read users from it.

Step 3j: agents on their own store. The `AgentManager` and the `AgentController` keep it and no
longer the facade; the `ChannelLoop` keeps it for whether a participant still exists
(`agent_exists`, `Member::of`), and the `ChannelRuntime` hands out `EntityStores` rather than the
facade; templates find and make agents in it, the views read them from it.

