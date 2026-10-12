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
  is made. How an error says what it is, so that `From` can answer it, is cut 12.

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

### Cut 12 - errors answered by what they are

**Before.** `ApiError::from_runtime(error, subject, action)` (`caelumora-rest-server/src/error.rs`)
downcasts an `anyhow::Error` to every error type of the runtime, and about 95 handlers call it with
a `Subject` (`Session`, `Agent`, `Participant`, `Cancel`, `Command`, `Context`, `Workspace`,
`Skill`, `Artifact`, `Group`) and the text of their action. The subject names the code of a refusal
(`<subject>_forbidden`) and of a server error (`<subject>_failed`); the action is in the detail
("Renaming skill … is not permitted", "Renaming skill … failed: …"). Both come from the handler,
not from the error: `StoreError::PermissionDenied` carries nothing, and `ViewError::Forbidden {
actor }` is `context_forbidden` in one handler and `agent_forbidden` in another. A plugin can add
neither its error types nor its subject, and an error of a plugin inside an `anyhow::Error` of the
core - `ScopeMember::delete_lying_in` answers `anyhow` - is not recognised at all.

| | Benefit | Cost |
|---|---|---|
| **A** `Subject` an open value (a constant per subject, a plugin defines its own); a mapping per kind next to its routes, falling back to `from_runtime` | API and runtime unchanged | handlers keep their `map_err`; subject and action stay with the handler; an error of a plugin inside an `anyhow` of the core is still a server error |
| **B** `From<…> for ApiError` per error type; the error carries what was refused | handlers use `?`; a refusal says what it is, wherever it is found, also in a plugin's error inside an `anyhow` of the core, and in the log | a change across model, store and runtime; a refusal's detail is the runtime's text, not the handler's |
| **C** a registry of mappings that `from_runtime` asks | handlers unchanged | `from_runtime` has no state: the registry would be global (`OnceLock`), hidden state |

**Decided:** B (2026-10-12). The information the handler adds today moves to where the error is
made. The design:

1. **A refusal is a value.** `Refused` in `caelumora_model::security`, a `thiserror` error without
   a source, `Clone`: `subject: &'static str` - the name the API gives what was refused, the
   prefix of its code - and `action: String`, what `ActionType::describe` names it; displayed as
   "{action} is not permitted". It is made by `ActionType::refused(&self) -> Refused`, whose
   subject is the kind's name for `Entity { kind, .. }` and `List(kind)`, and for the core's own
   actions: `ReadAgentContext` - `context`; `ReadAgentTools`, `ReadAgentComponents` - `agent`;
   `ApproveToolCall` - `approval`; `StopSession` - `session`; `CancelChannel` - `cancel`;
   `WakeChannel` - `channel`; `RetryMessage`, `ModifyMessage`, `RevokeMessage` - `message`;
   `AnswerQuestion` - `question`; `Reclassify` - `classification`; `ShowCommand`,
   `ExecuteCommand`, `ShowAgentCommand`, `ExecuteAgentCommand` - `command`; `CallTool` - `tool`;
   `ReadSecret` - `secret`.
2. **`Enforcement::require` hands it over.** Its refusal becomes `impl FnOnce(Refused) -> E`;
   `require` makes the `Refused` from the action it was asked. A caller that hides what it refuses
   as absent (`|_| StoreError::NotFound { .. }`, `|_| CommandError::Unknown(..)`) drops it. About
   50 callers in model and runtime change.
3. **Every error that means a refusal carries it as `#[source]`:**
   `StoreError::PermissionDenied(#[source] Refused)` (still displayed "permission denied"; the
   store's access check in `store::access` and the managers that refuse with it pass it on),
   `ChannelError::Forbidden { action, channel, #[source] refused }`, `CommandError::Forbidden {
   name, #[source] refused }`, `ViewError::Forbidden { actor, #[source] refused }`,
   `SecretError::PermissionDenied`, and the managers' own refusals that come from `require`
   (`WorkspaceError::ReadOnly`, …). Never `#[error(transparent)]` directly around a `Refused`: a
   transparent variant passes on its inner error's *source*, so the `Refused` would vanish from
   the chain; a transparent variant around an error that holds the `Refused` as its source - such
   as `SkillError::Store(StoreError)` - keeps it there. `caelumora_runtime::error::is_forbidden`
   becomes a search of the chain for a `Refused`.
4. **What a request names and does not find, the operation says itself.** Where the server asked
   `is_not_found(&error, kind)` - a session (opening and loading it), an agent, a workspace
   (attaching it), a session template (making a session) - the operation answers an error of its
   own that names it (e.g. `SessionError::NotFound(SessionId)`), not a `StoreError::NotFound` deep
   in an `anyhow`. A `StoreError::NotFound` reached on the way - a personality missing while a
   session is loaded - stays a server error, as today.
5. **The server answers by type.** `ApiError::code` becomes `Cow<'static, str>`. Every error type
   that reaches a handler has `From<…> for ApiError`, which answers each variant with the status and
   code `from_runtime` gives it today; a refusal is `403 {refused.subject}_forbidden`, its detail
   the error's own text. `From<anyhow::Error>` keeps the order of `from_runtime`: a `ValueError`
   anywhere in the chain; then the outermost error by its type, if it is one of the core's (so that
   `WorkspaceError::ReadOnly` stays `read_only`, though it carries a refusal); then
   `StoreError::NameTaken` and a `Refused` anywhere in the chain - the latter also in an error of a
   type unknown to the server, such as a plugin's -; and otherwise a server error: code
   `internal_error`, detail "The request failed: {user_message}" or, if `user_message` has nothing
   to disclose, the pointer to the log; the full chain is logged in the request's span, which names
   method and path. `Subject`, `from_runtime` and the action texts of the handlers go;
   `change_error(error, code)` keeps its own code for `StoreError::Validation`. The impls live in
   `error.rs`; those of a kind that becomes a plugin move with it (step 10: `From<SkillError>` into
   `caelumora-skills`, feature `server`).
6. **Handlers use `?`.** `.map_err(|e| ApiError::from_runtime(e.into(), Subject::…, "…"))?`
   becomes `?`.
7. **Codes.** Kept: every code the web UI reads - `context_forbidden`, `agent_forbidden`,
   `context_not_recorded`, `context_unavailable`, `command_not_found`, `workspace_not_found` - and
   every not-found, conflict and invalid code of today. Changed: a refusal whose code came from
   the handler's subject is named by what was refused (adding a participant, refused as an update
   of the channel, is `channel_forbidden`, no longer `participant_forbidden`); `<subject>_failed`
   becomes `internal_error`; a refusal's detail is the runtime's text.

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

### Step 8 - the server library, `caelumora-app`, the embedded UI, errors (cuts 9, 10, 12)

First part:

- `caelumora-rest-server` becomes a library: `auth`, `error`, the extractors, `state`, `sse` and
  `routes::core()` public.
- `caelumora-app`: `main.rs` from the server, the router nested under `/api`, `rust-embed` behind
  `web-ui`, `build.rs` as in cut 10, the fallback to `index.html`, the cache headers.
- `caelumora-web-ui`: the Vite proxy no longer rewrites `/api`.
- Documents: `ARCHITECTURE.md` (crates, composition root), `REST-SERVER.md` (`/api`, serving the
  UI, how a plugin adds routes), `WEB-UI.md` (building for the binary), `README.md` (how to build
  and run).
- Stored data: none. Deployments change their start command and base URL.

Second part - errors answered by what they are (cut 12), in this order, each stage compiling with
its tests passing:

1. Model: `Refused`, `ActionType::refused`, `Enforcement::require` handing it to its refusal,
   `StoreError::PermissionDenied(Refused)`, `store::access`; every caller of `require`; the tests
   that match `PermissionDenied` (about 30, the security suite among them).
2. Runtime: the errors of point 3 carry the `Refused` as `#[source]`; `is_forbidden` searches the
   chain for it; the four operations of point 4 report what they did not find in their own error.
3. Server: `ApiError::code` as `Cow`; `From<…> for ApiError` per error type, and for
   `anyhow::Error`; `Subject` and `from_runtime` removed; the handlers use `?`; `change_error`
   without a subject. The tests of `error.rs` become tests of the `From` impls, one per error type,
   plus one that finds the refusal of an error unknown to the server inside an `anyhow` (as a
   plugin's will be) and answers `403 <subject>_forbidden`.
4. The web UI is checked against the codes of point 7 (`tsc`, and live: the context, history,
   private notes and tools of an agent one may not read).
5. Documents: `REST-SERVER.md` (*Errors*: codes by what was refused, `internal_error`, how a
   new error type is answered), `CODING-GUIDELINES.md` (*REST errors*: handlers use `?`, an error
   type that reaches a handler implements `From<…> for ApiError`, a refusal is carried as
   `#[source]`), `SECURITY.md` and `PERMISSION-CHECKS.md` (`require` hands over a `Refused`).
6. Stored data: none.

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

Decided with the maintainer before the step was begun, each the recommended option:

- **Stores: the backend opens a kind's store generically.** `Stored` requires what every store
  needs of a kind (`Clone`, `Serialize`, `DeserializeOwned`, `Send`, `Sync`), so that a backend
  can open the store of any kind: `EntityStoreFactory::entity_store::<T>()`, implemented by
  `MemoryStore` and `JsonFileStore`, each opening a kind once and giving the same store whenever
  it is asked again - the core and every plugin share it. `EntityStores` keeps its fields for the
  core's kinds and is opened by the same method (`EntityStores::open`). The configured backend
  is `StoreBackend`, an enum of the two, which replaces the private `trait Store` of `config.rs`
  and gives the entity stores, the component store and the state store; the component store is no
  longer a second instance of the backend. `TypeMap` moves into the model, which keeps the
  opened stores in one. A plugin asks `RuntimeBuilder::entity_store::<T>()`, which refuses a kind
  whose name another kind has. Stores of a kind's own - tables and queries of a database - and a
  policy that a database can put into its query are issue [332](332-policy-as-a-predicate.md),
  not this step.
- **Services: one type map of the runtime.** `ComponentServices` becomes the runtime's
  `Services`: the policy, the secrets, the managers of the core that components use, and what a
  plugin `provide`s. Components reach a service as before (`ComponentKind::uses`,
  `ComponentContext::service`), the views by `Views::service`, a plugin's routes when they are
  built.
- **Sections: at the top level, an unclaimed one is an error.** `AppConfig` and `Seed` lose
  `deny_unknown_fields` and collect every key they do not know in a map (`serde(flatten)`). A
  plugin claims its configuration section while it registers (`RuntimeBuilder::config::<C>`);
  `build` refuses a section that nobody claimed, so that a misspelt section is still found. A
  plugin claims its seed section with a `SeedSection`, which is given the section's value;
  seeding refuses a section nobody claimed. Files keep their form.
- **Tests: a feature `test-support` of `caelumora-runtime`.** The `World` of the security suite,
  its recording policy and helpers move to `caelumora_runtime::testing`, public behind the
  feature; the suite and a plugin's tests use it as a dev-dependency with the feature.
- **Composition.** `RuntimeBuilder::new` makes what the core's managers need and the managers;
  a plugin's `register` adds its component kinds, services, scope members and sections; `build`
  sets the component defaults - which may name a plugin's kind -, checks the sections and makes
  the `RuntimeManager`. `RuntimeBuilder::from_config` makes everything the two programs made by
  hand from the configuration (the policy, the memberships, the stores, the secrets, the built-in
  registry); `RuntimeManager::new` stays as the builder without plugins. Seeding becomes
  `RuntimeManager::seed`, which knows the plugins' seed sections. `RestPlugin: Plugin` adds
  routes, built with the `AppState`; `routes::api` merges them behind the same authentication as
  the core's.

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
- An issue for the events of channels and of their members, which stayed variants in step 5: how
  a channel names its session in `GlobalEvent::Entity`, so that the web UI still finds it.

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

Step 3k: channels on their own store. The `SessionLoop` and the `SessionManager` make, restore,
delete and purge channels in it through their `EntityStores`; the `ChannelController` keeps it and
no longer the facade, the `WorkspaceManager` keeps it for where a workspace is used, and the views
of a session read channels from it. `Views::entities` is gone: the views read the facade only for
sessions now.

Step 3l: sessions on their own store, and the facade gone. The `SessionController` keeps the store
of sessions, the `WorkspaceManager` keeps it for where a workspace is used, the session code and
the views take it from `EntityStores`. `Entities`, `EntityStores::entities` and
`RuntimeManager::entity_store` are gone: every kind of the core is kept by an `EntityStore<T>` of
its own, and whoever uses a kind holds its store. Step 3 is done; the plugin of skills can make its
store as the backends make theirs.


**2026-10-12** — Step 4, on branch `issue-323-scope-member`: `ScopeMember` is the trait of a kind
that may lie in a scope (`kind`, `delete_lying_in`, `restore_lying_in`, `purge_lying_in`, each
failing with `anyhow::Error`), implemented by the agent, workspace, skill, artifact and group
managers, whose methods of the same names moved into it. Personalities and session templates,
which have no manager, are members by their store: `Unowned<T>`, generic over the kind, in place of
the two inline loops. `ScopedEntities` keeps the members in a list, made by the runtime in the order
of before, and the `GroupManager` beside it, since the rule that groups go last, and only once
nothing else is left, is the scope's. Groups are now purged after all others too, no longer between
skills and workspaces; nothing purged depends on that order. `restored_unless_refused` takes what
was deleted and logs its kind and id instead of a kind written out by each caller. Nothing stored
changed.

**2026-10-12** — Step 5 decided with the maintainer, on branch `issue-323-entity-event`. The plan
named the kinds that may become plugins; that line said nothing about the events, and the
maintainer asked why agents and users were left out. The line drawn instead is the event's own:
every kind whose events name it by its own id - sessions, agents, users, user profiles,
workspaces, skills, artifacts, groups - is announced as `GlobalEvent::Entity { kind, id, change }`;
channels and the events of their members keep their variants, since they name the session too. A
custom change (`EntityChange::Custom`, as `Verb::Custom` for actions) carries what is particular to
a kind - `session::STARTED` and `STOPPED`, `agent::STATE_CHANGED` - so that a session's events are
not split over two forms, and a plugin can announce what is no lifecycle event without option B of
cut 5. Since the events are what instances will tell each other (issue 122), `GlobalEvent` stays
`Serialize` and `Deserialize`: an update is carried as a JSON value, read the same way whether it
was published here or elsewhere (`change.update::<UserUpdateEvent>()`), and the name of a kind and
of a custom change are interned as they are read. A typed payload behind a trait was weighed and
dropped for that reason; `typetag` too, since an event would be unreadable as a whole wherever one
type is not registered.

Done: `GlobalEvent::created`, `updated`, `deleted`, `restored` and `custom` make an event by its
kind, `about::<T>()` reads it back, with the id parsed from its text (`Stored::Id: FromStr`, which
`define_id!` now gives). `read_action` is one rule for every entity, the channel's for the rest;
the topics of sessions, agents and users are found by `about`. Artifacts announce their first
version as created and the others, their access, scope and description as an
`ArtifactUpdateEvent`, as the other kinds do. The controllers, the session loop and
`changes_who_may_run` read the new form; the web UI reads an entity event by its kind
(`entityEvent`), and its trash follows a deletion or restoring of any kind. Nothing stored changed.

**2026-10-12** — Step 6, on branch `issue-323-knowledge`. `KnowledgeSource::Entity { kind, id,
digest }` replaces `Skill` and `Workspace` alike - decided in this step, since a workspace's files
are the same shape with an optional digest, and workspaces are to become a plugin as well -, made
and read by kind (`KnowledgeSource::entity::<Skill>`, `about::<Workspace>()`), as the events are.
The id is a `Uuid` here, not text as in an event: knowledge is stored with every message and stays
`Copy`, and every kind's id is one. `KnowledgeRestorer` is a facet in `crate::compaction`: it names
its kind and answers which of the entities recorded as given it gives again, and as what text; the
`skills` component implements it, and `summary_compaction` takes the restorers of the agent's
components when it is built, asks each for its kind, and knows none itself - also not in what it
asks the model to leave out, which names the restorers' kinds. `Compaction` tells `given_again` by
kind and name and how much was `lost`; `CompactionError::KnowledgeUnreadable` replaces
`SkillsUnreadable`. `ToolPresentation` is a name, written as before; its values are constants next
to the tools (`SHELL_PRESENTATION`, …), `DEFAULT` its own. The REST server tells knowledge of an
entity by its kind as `source` and its `entity_id` in place of `workspace_id` and `skill_id`,
which the web UI did not read; it names a kind it does not know as such rather than as a history.
Stored data: the state store has to be deleted - transcripts and histories hold `KnowledgeSource`
in its old form.

**2026-10-12** — Step 7, on branch `issue-323-rest-api`. The crate `caelumora-rest-api` holds the
request and response types, one module per subject, and depends on the model alone; `models.rs`
is gone. The plan had the conversions from the snapshots move into the server; they stay with the
types, since the snapshots are the model's (`caelumora_model::snapshot`) and the server could
implement `From` for neither side. What needs the runtime moved into the handlers: the knowledge
of a message named after its channel and agent (`knowledge_response` in `routes::channels`), a
file of a workspace and a command of its shell (`routes::workspaces`). Two things the wire named
of the runtime moved to the model: `CommandStatus` (`caelumora_model::workspace`), a value a
command's state and the API share, and `LabelCauses` (`caelumora_model::confidentiality`), the
rule which classifications set a message's label. The JSON of the API is unchanged. Stored data:
none.

**2026-10-12** — Step 8, first part, on branch `issue-323-app`. `caelumora-rest-server` is a
library (`lib.rs`; `auth`, `error`, `routes`, `sse`, `state` public; `routes::core` in place of
`create_router`); resolving the signing key moved from `main.rs` to `TokenService::from_secret`,
and `LoginRequest` and `UserResponse`, left in the server by step 7, to `caelumora_rest_api::auth`.
`caelumora-app` is the program: `main.rs` from the server, the API nested under `/api`, where a
path no route takes is `404 not_found` as a problem, the web UI from `web_ui.rs` behind the
feature `web-ui` with `index.html` as the fallback and the cache headers of cut 10, and `build.rs`
as cut 10 has it. The Vite proxy keeps `/api`. Checked in the browser against the program: the
page, logging in, the lists of the sidebar, a channel and both event streams under `/api`.

Making each error type answer itself turned out to need more than the plan said: a `From` sees
only the error, and today the handler, not the error, says what was refused and in which action.
The options are cut 12; the maintainer decided B, the refusal carried by the error, as the second
part of this step.

**2026-10-12** — Step 8, second part (cut 12), on branch `issue-323-app`. `Refused` and
`ActionType::refused` are in `caelumora_model::security`; `Enforcement::require` hands the refusal
to the caller's error, and `StoreError::PermissionDenied`, `ChannelError::Forbidden`,
`CommandError::Forbidden`, `ViewError::Forbidden`, `WorkspaceError::ReadOnly` and
`SecretError::PermissionDenied` carry it as their source. `caelumora_runtime::error::refusal` finds
it in a chain; `is_forbidden` asks it, and `is_not_found` is gone. The `SessionManager` answers
what a request named and it did not find as `SessionError::NotFound` and `TemplateNotFound`. In the
server `ApiError::code` is a `Cow`, every error type has its `From<…> for ApiError` in `error.rs`,
and the about 90 handlers that called `from_runtime` pass their errors on with `?`; `Subject` and
`from_runtime` are gone. In `From<anyhow::Error>` the order of `from_runtime` is kept - the
outermost error by its type before a refusal anywhere in the chain - so that
`WorkspaceError::ReadOnly` stays `read_only`; the plan said otherwise and is corrected. Checked
against the program: an unknown session and template (`404 session_not_found`,
`template_not_found`), a global skill renamed by another user (`403 skill_forbidden`), a blank
name (`400 invalid_value`), an unknown command. `context_forbidden` and `agent_forbidden` are
covered by tests only: making a user who may not read an agent's context a member of its channel
needs a template that names them. Stored data: none.

**2026-10-12** — Step 9, on branch `issue-323-plugin`, after the decisions recorded under the
step. The store: `Stored` requires `Clone`, `Serialize`, `DeserializeOwned`, `Send` and `Sync`;
`TypeMap` moved into the model (`type_map`); `MemoryStore` and `JsonFileStore` open the store of a
kind when it is first asked for (`EntityStoreFactory`) and keep it in a type map, so that their
eleven fields per kind are gone and `EntityStores::open` makes the core's stores the same way;
`StoreBackend` replaces the private `trait Store` of `config.rs`, gives the entity, component and
state stores and refuses a kind whose name another kind has; `AppConfig::open_entity_store`
replaces `create_entity_stores` and `create_component_store`, so that the component store is no
second instance of the backend. The runtime: `Plugin` (`plugin.rs`); `RuntimeBuilder` in
`runtime.rs` with `new`, `from_config`, `plugin`, `entity_store`, `provide`, `service`,
`components`, `scope_member`, `config`, `seed_section` and `build`; `RuntimeManager::new` builds
without plugins and returns `anyhow::Result`; `ComponentServices` is `Services`
(`RuntimeManager::services`); `ScopeMember` and the module `scoped` are public; seeding is
`RuntimeManager::seed`, and a `SeedSection` seeds a plugin's section. `AppConfig::plugins` and
`Seed::plugins` collect the sections nobody else knows; one that no plugin claims is refused when
the runtime is built or the seed applied. `Views` opens `context`, `security`, `confidentiality`,
`service` and `channel_reading`; the stores by kind, which the plan named, are not opened: a
plugin's views read through its manager, the one owner of its kind. The server: `RestPlugin` and
`routes::api`, which replaces `routes::core` and serves a plugin's routes behind `require_auth`.
`caelumora-app` and `caelumora-tui` build through `RuntimeBuilder::from_config` with an empty list
of plugins. The world of the security suite is `caelumora_runtime::testing` behind the feature
`test-support`, which the runtime takes as a dev-dependency of itself. `define_id!` writes its
`Display` out, so that a plugin using it needs no `derive_more`. Tests: a plugin with a kind of its
own, its manager as a service reached by a component and the views, its sections of the
configuration and of the seed, a section nobody claims, one claimed twice, a kind named as the
groups; a plugin's route behind the login; the backend's stores per kind. Checked against the
program: the API and the UI under the live configuration, a session made, deleted and restored, a
misspelt section of the configuration and an unclaimed section of a seed refused at startup, the
TUI started. Stored data: none.
