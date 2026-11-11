# 554 — The terminal program keeps its data as a server does, out of reach of its user

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Persistence
**Summary:** `caelumora-tui` keeps everything in directories it is configured with, in files named by UUID, and works in workspaces of its own, so that on one person's machine nothing it does can be told by the files in their home, and none of its sessions can be found or read there.

## The problem

`caelumora-tui` runs the kernel in its own process, as a program on one person's machine - the shape
of a tool like Claude Code. Its data is laid out for a server:

- The backends `json` and `postgres` keep a document by its id: `entities/<kind>/<uuid>.json`. A
  skill or a personality cannot be written by hand into a directory, found there by its name, or
  put under version control as a dotfile.
- What belongs to one session is spread: the session and its channels in `entities/`, their states
  in `runtime/channel_state/`, `runtime/agent_channel_state/` and `runtime/user_channel_state/`, the
  content of attachments and artifacts in a storage of content. A session cannot be looked at,
  copied or removed as one directory.
- A workspace is a directory below a storage's `root`, named by its id
  (`caelumora-kernel/src/terminal/local.rs:111`). The program cannot work in the directory it was
  started in.
- The paths are the configuration's, relative to the working directory, and the log is fixed to
  `local/tui.log` (`caelumora-tui/src/main.rs:587`).

## Why it matters

The two programs are different applications that share one mechanism. The same data need not be
readable by both: what is wanted is the kernel, with its sessions, channels, agents, tools and
approvals, behind a layout that fits a home directory. The multi-user server stays as it is.

## What does not matter here

For a program of one person, two subjects have no meaning:

- **Permissions between users.** There is one. The configuration `security: allow_all` exists for it
  and is the program's default: every check still runs and answers yes, so that no code forks and a
  file names no owner and no access list. What is decided for agents stays: approvals are no part
  of the access policy, and the rules of confidentiality hold under `allow_all` as well. That the
  policy is called development-only (`caelumora-security/src/policies.rs`) is reworded.
- **Audit.** So what is deleted is purged at once, and a person may change a file by hand although
  no writer announced it - personalities and skills are read afresh when they are used. Chains and
  what an agent was given (`<agent>.given/`) are kept all the same: leaving them out would be code
  of its own, and they go with their session. Resuming a turn needs what it names of the latter.

## The rule: a name is an address, an id an identity

Decided with the maintainer, and to be written into `docs/STORAGE.md` under *Names and scopes*:

> **A name is an address, an id an identity.** Definitions and configuration may address what they
> refer to by name; the address stays as written and is resolved when it is used. What is recorded
> of a resolution is the id: what is state - a session, a channel, an agent, a history, a
> transcript - refers by id alone. Who owns something, who may reach it and where it lies are
> identity, also within a definition.

So:

- **By name:** a personality's `ref` to a bundle, as today; a template's agents, as today, and its
  personalities, which is new; `Channel::components` and `Session::commands`, which are
  configuration held by state, and stay as written.
- **By id:** `Agent::personality_id`, participants, a turn's knowledge, history, transcript,
  attachments; owner, access list and scope everywhere.

`@name` stored as an `ActorId` is the same shape: a name is said, and the id is what is kept.

## Where everything lies

```
~/.config/caelumora/                 what the person writes ($XDG_CONFIG_HOME)
  config.yaml
  skills/research.md                 front matter, the body as Markdown
  personalities/team-coder.yaml
  bundles/agent-basics.yaml
  templates/team-session.yaml
~/.local/share/caelumora/            what the program writes ($XDG_DATA_HOME)
  agents/
    <agent-uuid>/                    the agent and its memory
    coder -> <agent-uuid>
  projects/<project-uuid>/           shared projects, with their artifacts
  sessions/<session-uuid>/
    session.json
    lock
    agents/  skills/                 what lies in the session's scope
    projects/<project-uuid>/         its own project, with its artifacts
    channels/
      <channel-uuid>/                document, state, history, attachments,
                                     agents/<agent-uuid>.*
      general -> <channel-uuid>
```

- **The place follows the scope** (`Document::name`). The backend knows kinds by their name alone,
  so that a plugin's kind needs no code of it: a table says of a kind its directory and its format.
- **Definitions are files named by their name**: `component_bundle`, `personality`,
  `session_template`, `skill`.
- **State lies in directories named by its id**, also where it has a name, since a channel, an
  agent, a project or a workspace can be renamed. Moving a directory while actors write below it -
  for an agent in every channel of every session, also of other running programs - cannot be done
  in one step. A name is a symbolic link beside the directory, replaced atomically on a rename, and
  **the backend never reads one**, so that one out of date or missing does no harm, and a platform
  without them loses nothing but the label. An agent has one under `agents/`, none within a channel.
- **Global agents lie in the data directory**: they have memory, and nobody writes them by hand.
- **The shell is bash**, which `LocalShell` already is; another shell would be another `Shell`.

## Identity of a file

All 14 kinds of `Stored` are kept by UUID (`DocumentStorage::get(id: Uuid)`), and the kernel stays
so. The mapping is the backend's alone, and only definitions need one:

| Kinds | Where the id comes from |
|---|---|
| `skill`, `personality`, `component_bundle`, `session_template` | the field `id` of the file, or derived |
| `agent`, `channel`, `workspace`, `project`, `artifact`, `group`, `user` | the document holds it, its directory is named by it |
| `session`, `user_profile`, `memory` | a session's directory; the others are the ids of their user and agent, as today |

- **Name to id:** a file without an `id` has `Uuid::new_v5(NAMESPACE, "<kind>\n<ScopedName::key>")`,
  a pure function, the same after every start and on every machine. A test pins the id of one name,
  so that a change of `fold`, of which `ScopedName::key` warns, cannot go unnoticed.
- **Id to path:** a derived id cannot be inverted. The backend reads the directory of the kind and
  holds a map in memory, a cache never stored, read again on a miss, so that it sees what another
  process wrote. For state the path is computed; which session a channel lies in is told by the
  names of the directories, without reading a file.
- **Reading** sets `id` and `name`, the latter from the name of the file. **Writing** leaves the
  `id` out only where the file had none and the id is the derived one: an `id` once in a file stays.
- **`mv` of a file without an `id` is a new identity**, decided with the maintainer. References by
  name follow the new name as any address does. With an `id` in the file, references by id survive
  it. The one reference by id that hurts is `Agent::personality_id`: an agent that lives long loses
  its personality, and a template then fails with `AgentNameTaken`. The message of such a dangling
  reference names the UUID, which goes as `id` into the file renamed, and the reference holds again.
- **Two files of one id** - a copy with its `id`, or a new file under the old name of one renamed
  that kept the derived id - are both left out and reported with both paths: which of them won
  would depend on the order of a directory. The latter needs an `id` of its own. **Two files of one
  name** as names compare (`Zoë.md`, `zoe.md`) are treated alike.
- **`caelumora id <file>`** prints the UUID a file has, and whether it is derived or its own. A
  template needs it, or a personality with an `id`; one that a template refers to should have one.
- **A name in a file's name** is encoded reversibly (`/`, `%`, a leading `.`). `Name` allows up to
  100 characters of up to four bytes; what comes to more than 255 bytes is refused.
- **Other fields** of a file - `revision`, a scope other than the global one - are written into it
  where they differ from the default, with no file beside it. A file written by hand has none: as
  STORAGE.md says of seeds, a document written by hand needs no revision.

## Several programs at once

Two terminals are the normal case, and what the stores assume today does not hold then:

- `JsonDocuments` locks within its process, "the only one that writes the files"
  (`caelumora-store-json/src/lib.rs:413`), where the contract of `DocumentStorage` asks for holding
  against every writer. The new backend takes a `flock` per kind.
- A session runs in one instance. `sessions/<id>/lock`, a `flock` held while it runs, keeps that so.
  `SessionManager::resume_interrupted` is called by `caelumora-app` alone, so the terminal program
  starts only what it is told to.
- There is no event bus between the programs (issue [122](122-several-instances.md)): one learns of
  what another changed when it next reads. An agent's memory is safe by its revision.

## Considerations

- **A new crate, over a second layout in `caelumora-store-json`.** Of the JSON store's almost 1500
  lines without tests, the state side - journals, chains, items, transcripts - stays the same but
  for about ten functions that make a path and two walks of a directory. `JsonDocuments`, almost 200
  lines, shares nothing with files by name. So the JSON store gives its state side a layout, and the
  new crate builds on it, while the JSON store stays one sentence. CRATES.md lets a plugin depend on
  another only for a backend of that plugin's service; the rule is extended by a backend that builds
  on another.
- **Importing the files into the JSON store at start** was rejected: two truths, and a skill an
  agent writes would never reach the files.
- **Directories of state named by name** were rejected, as above.
- **A required `id` in every file**, as `users.yaml` asks (issue
  [25](25-file-user-backend.md)), was rejected: every file written by hand would hold a UUID. Issue
  25 rejected derived ids because a renamed user leaves every ownership reference behind; here,
  renamed is a personality, with one reference, and an `id` remains possible.
- **Issue [93](93-entity-store-dialects.md)** made templates kept by id and took the lookups by name
  away for one dialect of the stores, no argument against names as references. Issue
  [455](455-components-configuration-kept-as-entities.md) kept the `ref` by name, its plan giving as
  the reason that a UUID in YAML is hard to read. On the server only the seed writes personalities
  today, so that renaming one, which breaks a template visibly at the next session made from it,
  hardly arises.

## Interfaces

Step 1, in `caelumora-store-json`:

```rust
/// Where the states of channels, agents and users lie.
#[async_trait]
pub trait StateLayout: Debug + Send + Sync {
    /// The snapshot of the state of `channel`; its journal and its history lie beside it.
    async fn channel_state(&self, channel: ChannelId) -> StoreResult<PathBuf>;
    /// The directory of the states of the agents in `channel`, those of each beside each other.
    async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf>;
    /// The directory of the states of the users in `channel`.
    async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf>;
    /// What `agents_in` answers, of every channel.
    async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>>;
    /// The directory of the index of the work under way.
    fn under_way(&self) -> PathBuf;
}

/// The states, their histories and transcripts in JSON files, where a layout puts them.
pub struct JsonStates { /* … */ }
impl JsonStates {
    pub fn new(layout: Arc<dyn StateLayout>, journal_floor: u64) -> Self;
}
// impl StateStore, HistoryStore, TranscriptStore for JsonStates
// JsonFileStore: its documents as today, its states a JsonStates with the layout of today
```

Step 2, in `caelumora-domain`:

```rust
pub enum ParticipantTemplate {
    User { name: String },
    /// An agent of the personality `personality`, found by its name in `scope` or made.
    Agent { name: Name, scope: ScopeTemplate, personality: ScopedName },
}
// ParticipantRequest::Agent { name: ScopedName, personality: ScopedName }
```

The kernel resolves it by `find_by_name`, in exactly that scope, global unless named, as a `ref` is.
The agent stores what it found, `personality_id`.

Step 3, in `caelumora-artifacts`: the key of a version is
`artifacts/<project>/<artifact>/<content>`. The content is kept before the document exists
(`manager.rs:240`), so at its first publication the artifact cannot tell the backend where it lies;
its project can.

Step 4, the new crate `caelumora-store-home`:

```rust
/// The parameters of the backend `home`.
pub struct HomeStoreConfig {
    /// What the person writes: the directories of the definitions.
    pub config_dir: PathBuf,
    /// What the program writes.
    pub data_dir: PathBuf,
    /// Which kinds are definitions, and how each is written; any other is state.
    pub definitions: BTreeMap<String, DefinitionFiles>,
}

/// How the definitions of one kind are written.
pub struct DefinitionFiles {
    /// Their directory, below `config_dir`, or below the directory of their scope.
    pub dir: String,
    pub format: FileFormat,
}

pub enum FileFormat {
    Yaml,
    /// Front matter, and the field `body` as the text after it.
    Markdown { body: String },
}

/// The stores of a program of one person (`PersistenceBackend`), `type: home`.
pub struct HomeStore { /* … */ }

/// The content of `home` (`ContentStorage`): what lies under the second segment of a key - a
/// channel, a project - lies in the directory of that.
pub struct HomeContent { /* … */ }

/// The backends `home` of the stores and of the content, sharing where things lie.
pub struct HomeStorePlugin;
```

## Steps

Each reviewed and committed on its own, on a branch of this issue.

1. Without a change of behaviour: `JsonStates` and `StateLayout` out of `JsonFileStore`.
2. A template names a personality by name; the rule goes into `docs/STORAGE.md`, and CRATES.md is
   extended. Stored data: the session templates in the stores, to be deleted; the seed changes.
3. The key of an artifact's content holds its project. Stored data: the content of artifacts.
4. `caelumora-store-home`: documents, state, content, the mapping of ids, `flock`, the session's
   lock; run through the store suite.
5. The storage of workspaces of the directory the program was started in, in the kernel beside
   `local`, whose `LocalTerminal` and `LocalShell` it uses. `remove` never removes the directory.
   Its `destination` has to be set, as a storage with a shell is otherwise bounded to public. To be
   settled in its plan: how a workspace finds its directory. A UUIDv5 of the path needs
   `WorkspaceManager::create` to take an id; a symbolic link `<root>/<id>` to the directory, made by
   `create` and removed by `remove`, needs no change there and leaves `place` as it is, and the
   workspace is found by a name made from the path. The latter is preferred.
6. The terminal program: its defaults (`security`, `accept_any` with the name of the user of the
   system, `home`, the storage of step 5, the paths of XDG, the log), `--resume`, sessions listed by
   directory, `caelumora id`.

## Done when

- Started in a directory with no configuration, `caelumora-tui` works in that directory, with what
  lies in `~/.config/caelumora` and `~/.local/share/caelumora`.
- A skill written as a Markdown file and a personality as a YAML file are used by their names,
  without a UUID in the file.
- A session is one directory; removing a session removes it.
- Renaming a channel or an agent moves nothing.
- Two programs at once neither corrupt anything nor run one session twice.
- `caelumora-store-home` passes the store suite; the server's backends behave as before.

## History

- **2026-11-10** — Opened after a discussion with the maintainer, which decided: the backend, not a
  second kernel; the rule of addresses and identities; `mv` of a file without an `id` as a new
  identity, an `id` in a file taking precedence; `Agent::personality_id`, owner, access list and
  scope stay ids; configuration held by state stays by name; a crate of its own over the state side
  of the JSON store; directories of state by id, with names as symbolic links.
- **2026-11-10** — Step 1: `JsonStates` and `StateLayout` out of `JsonFileStore`, which no longer is
  a state store itself but hands out its `JsonStates`; a layout says where alone, and what is
  written makes its directories. `JsonFileStore::new` takes the journal's floor in place of
  `with_journal_floor`. To be settled in the plan of step 4: the store suite puts states of channels
  that have no document, for which the layout of `home` knows no session.
- **2026-11-10** — Step 2: a template names a personality by name, resolved as the session is made,
  in the name of whoever makes it and before anything is stored; what an agent keeps is its id. The
  rule of addresses and identities is in `docs/STORAGE.md`, and `docs/CRATES.md` allows a plugin
  that builds a backend on another's. Stored data: the session templates, to be deleted.
- **2026-11-10** — Step 3: the key of an artifact's content holds its project,
  `artifacts/<project>/<artifact>/<content>`. An artifact never leaves its project: lifting one
  keeps its id. Stored data: the content of artifacts and their documents, to be deleted.
- **2026-11-10** — Decided for step 4, with the maintainer. **The backend finds what lies where by
  searching, and remembers it.** `DocumentStorage` and the state stores name a document or a channel
  by its id alone, and only this layout needs what lies above it. Passing the session down
  (`Address<Channel>`) was weighed and rejected: it costs 21 methods of the kernel's stores, and
  leaves what lies in a scope, which no address names, to be searched for all the same. `Address`
  stays what issue 335 made it, how an event names an entity. A channel without a directory of a
  session lies in `channels/<id>`, for the store suite, which keeps states of channels no document
  names. **A document stays where it was made**: a project is tied to the directory the program was
  started in, so that nothing is lifted, and should a scope change all the same, the document says
  so and nothing moves. **A session's artifacts lie in its project**, `projects/<id>/artifacts`, not
  in the session's directory, and outlive it. Sessions stay flat, `sessions/<id>`, since the kernel
  makes each in its own project and moves it from there. **`memory` and `user_profile` lie in the
  directory of their agent and user** (`HomeStoreConfig::within`), since the backend knows kinds by
  name alone. **Comments in a file are lost** when the program writes it. The session's lock moves
  to step 6: only the program knows when a session runs. Step 4 is committed in three parts: the
  crate with its documents and states; the definitions as files; the content and the plugin.
- **2026-11-10** — Before step 4, without a change of behaviour: `JsonDocuments` and
  `DocumentLayout` out of `JsonFileStore`, as step 1 did with its states. Documents by id lie as
  JSON files in `home` too, so that it would otherwise repeat their revisions and names; what is
  written makes its directories.
- **2026-11-10** — Step 4, its first part: the crate `caelumora-store-home`, its documents by id
  and its states, a `flock` per kind, passing the store suite. A document is the file `<kind>.json`
  in a directory named by its id, in `<kind>s` of whatever its scope says. The states of a channel
  lie in `state/` of its directory, apart from what lies in the channel's scope. Removing a document
  removes what it leaves empty, never what is still in its directory. `HomeStorePlugin` comes with
  it, which the program links in step 6. Still to come: the symbolic links of names, which need the
  layout to hear of a write; and whether the session's lock of step 6 also keeps another program
  from purging a running session, whose channel would otherwise start a directory of its own.
- **2026-11-10** — Before the second part of step 4, as a preparation: `JsonDocuments` is
  `FileDocuments`, since how a document looks in its file is a `DocumentFormat` of its own, and a
  `DocumentLayout` says only where it lies. It offers `all` as a stream, `named` to narrow what a
  name is looked for among, `moved_to` and `written`; `lock` names the file a write holds the lock
  of, in place of `writing`. One change of behaviour: the JSON store's writes are kept apart by a
  lock of the file system, `entities/<kind>.lock`, also between processes. `scan` still takes every
  id first: what does not meet a filter is passed over for what follows, so that how many are
  needed cannot be told, and a bounded selection would list the directory anew within one scan.
  A format is given the path, since a definition takes its name and its id from it.
- **2026-11-10** — Step 4, its second part: the definitions as files and the links of names. A
  definition of a user's or a group's scope lies in the directory of that user or group in the data
  directory, not in the configuration directory with its scope in it: the names of different
  scopes would share a file there. A deleted definition lies by its id in `.deleted`, so that its
  name is free, as the store asks of what is deleted. Looking one up by its name reads every file of
  the kind: whether another file holds the same id or name is told by reading them all, also for
  one looked up by its id, so that a copy made meanwhile is seen. One type of `home`,
  `Definitions`, is the layout and the format of a kind's definitions alike. A file named by a
  name takes it from its file, whatever a field says. No link is made for what lies in a user's or
  a group's scope, whose names share a directory, nor under a name that is an id. What prints the
  id of a file goes public with `caelumora id`, in step 6.
- **2026-11-10** — `DocumentFormat::render` is given the text of the file it takes the place of,
  which `FileDocuments` read to judge the write: an `id` once in a definition's file stays when the
  file moves - deleted, restored, renamed -, and a format reads no file itself.
- **2026-11-10** — Step 4, its third part: the content `home` (`HomeContent`), registered by
  `HomeStorePlugin`. A key lies in `content/` of the directory of what its second segment names,
  apart from the documents there - the versions of an artifact would otherwise share the directory
  of its document, which removing them would take along -, where the key space is named in
  `spaces`, found by the same search as the stores' - decided with the
  maintainer, who finds none of the ways satisfying; issue
  [556](556-what-an-id-lies-in-is-found-by-searching.md) thinks it through. The files are written
  by the kernel's `LocalContentStorage`, now public for it, below the data directory, so that a
  copy between two channels is one of one storage.
- **2026-11-10** — Step 5, decided with the maintainer and done: the storage of workspaces
  `directory`, `LocalProvider` whose workspaces all lie in one directory (`dir`), each named by a
  link `<root>/<id>` to it, made by `create` and removed by `remove`, the directory never. What
  works in a workspace - its terminal, a command and its environment - is handed the directory
  itself, never the link, so that `pwd` and the paths a model is shown are where it works. Its
  `destination` has to be given. `environment: inherit` hands a command the program's environment,
  its `HOME` too, so that the person's tools, configuration and keys are there; `isolated`, the
  default, is `local`'s. A workspace is found by a name made from its directory's path, which
  step 6 settles.
- **2026-11-11** — Step 6, decided with the maintainer: in three parts - the program starting
  without a configuration in its directory; the sessions of a directory (`--continue`, `--resume`,
  `--sessions`) under the session's lock; `id <file>`, with the program named `caelumora`.
  `~/.config/caelumora/config.yaml` lies over the defaults, section by section; `--config`
  replaces them. A first start writes a template `default` and its assistant's personality into the
  configuration directory, so that they are the person's to change.
- **2026-11-11** — Step 6, its first part: the program's directories of XDG, its defaults
  (`allow_all`, `accept_any` with the system's user, `home` for the stores and the content, the
  storage `here` of the directory it was started in, with a shell and the person's environment),
  the files of a first start, and the project and the workspace of the directory, found by its name
  and a part of an id of its path, or made in the user's scope. Started for real: an empty home
  gets its files, a second start in one directory finds its project, workspace and agent again, and
  another directory gets its own. The links of `directory` lie in `links`, since `workspaces` of the
  data directory holds the documents of the workspaces, and a link there met one.
- **2026-11-11** — Decided before the second part of step 6: finding or making a workspace and a
  project is their managers' (`find_or_create`), shared by templates and the terminal program, and
  what a caller chooses of a new entity is a `…Spec` that `create` takes. The specs come first, as a
  step of their own without a change of behaviour: `WorkspaceSpec`, `ProjectSpec`, `AgentSpec` and
  `GroupSpec`, and `ChannelSpec` with a `new` and its defaults in place of `None`; each in the
  `spec.rs` of its module, `SessionSpec` and `ChannelSpec` in that of `session`.
- **2026-11-11** — Finding or making: `WorkspaceManager`, `ProjectManager` and `AgentManager` find
  one by the name a spec asks for, or make it, and look once more for one made by another meanwhile
  (`find_or_create`, the steps shared as `found_or_made`); a workspace of that name in another
  storage and an agent of another personality are refused by their managers' errors, which take the
  place of the template's. Templates and the terminal program use them: three programs started at
  once in a new directory get one project and one workspace.
- **2026-11-11** — Step 6, its second part, decided with the maintainer: first the session's lock,
  as a step of its own, then `--continue`, `--resume` and `--sessions`, the sessions of a directory
  ordered by their last message. The lock is the store's to offer and the `SessionManager`'s to
  take (`SessionLocks`, `PersistenceBackend::session_locks`): held as a session starts and let go as
  it stops, and taken for its deletion, restoring and purge where it does not run, so that another
  program neither starts, deletes, restores nor purges a running session (`RunsElsewhere`) - which
  answers the question of the first part of step 4. `home` holds a session by a `flock` of
  `locks/sessions/<id>`, not of `sessions/<id>/lock`, which would keep the session's directory from
  being removed with it; the file goes as the session is purged.
