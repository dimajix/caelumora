# 607 — An agent's notes are built into the kernel, not brought as a system service

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Context and knowledge
**Summary:** An agent's notes and private notes are built into the domain, the API, the kernel, the security policy and the agents' library - chains in `AgentChannelState`, fields of `TurnState` beside the generic staging, variants of `KnowledgeSource` and `Reclassified`, views and a command of the kernel - while the agent's memory, skills and artifacts are system services that a plugin and a library bring in a crate of their own.

## The problem

Notes ([17](17-information-sharing-within-session.md), [19](19-memory-tiers.md), principles 2 and
3) and private notes ([51](51-agent-private-notes.md)) are a subject of their own, but no crate owns
them. They are spread over every layer:

- **Domain:** `note.rs` (`NoteId`, `NoteOrigin`, `NoteChain`), the chains `notes` and
  `private_notes` in `AgentChannelState` and the changes staged for them in `TurnState`, both taken
  over by `AgentChannelState::finish_turn`; `KnowledgeSource::Notes` and `PrivateNotes`;
  `PublishedNotes`, `Detail::NOTES` and `ChannelParticipantSnapshot::notes` in the snapshot.
- **API:** `ChannelReader::notes`, `SessionReader::own_private_notes`, `PrivateNotesElsewhere`,
  `ActorRuntime::may_reclassify_note`.
- **Kernel:** `ChannelView::notes`, `ParticipantView::notes` and `private_notes`,
  `SessionView::own_private_notes` and `private_notes_of`, the command `/channel notes`
  (`command/channel/notes.rs`), the announcement `channel::AGENT_STATE_CHANGED`.
- **Security:** `Reclassified::Note`, answered by the ownership policy by authorship.
- **Agents:** `AgentNotes` and `NoteKind` (`notes.rs`), `Turn::stage_note`, `staged_notes` and
  `may_reclassify_note`, the components `notes` and `private_notes`.
- **REST:** `GET …/channels/{cid}/notes`, `GET …/actors/{aid}/private_notes`, the notes in a
  channel's snapshot (`ChannelResponse::notes`).

The agent's memory was in the same place until
[518](518-memory-is-reached-through-the-agents-state.md) made it a system service of
`caelumora-memory`. That issue kept the notes where they are: "Notes and private notes keep their
fields of `TurnState`: they change the agent's state in its channel, which the agent side holds and
stores itself, not a system service - a deliberate difference, to be revisited once a second service
stages" (its decision 3, "Two ways to stage").

## Why it matters

- [606](606-no-notes-belong-to-the-channel.md) asks for notes that belong to the channel, in a crate
  of their own - a plugin that keeps and decides, a library with the component. Built beside the
  notes of today, the crate would hold one kind of notes and the kernel another, with two ledgers of
  notes, two boards, two ways to stage, and the question whether they replace each other answered
  across crates. Moved first, the notes of today and those of the channel are one subject in one
  crate, and the choice between them is cheap either way.
- The kernel, the domain and the security policy know a subject that only an agent's components
  use, against the rule that a plugin "adds nothing about its kind to the kernel"
  ([CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#rules-for-every-change)).
- A turn stages in two ways - `Turn::stage` for the system services, `stage_note` for the notes -,
  which 518 accepted until a second service stages.

## Considerations

What holds and must keep holding - the move changes no behaviour:

- Notes belong to the agent that wrote them and are published in their channel; others read the
  chains as they stand, only of finished turns ([19](19-memory-tiers.md), principles 2 and 3).
  Only the author changes them. Private notes stay an alternative to memory, chosen per
  personality ([68](68-notes-in-agent-state.md)).
- The board shows the notes of the agents taking part; one that leaves, is removed or deleted takes
  its notes off the board and gets them back when it returns or is restored; purged, they go with it
  ([581](581-the-notes-of-an-agent-that-left-fall-off-the-board.md)). The first reason given there,
  "Notes live in the agent's `AgentChannelState`", no longer holds afterwards; the decision does.
- A channel read entry by entry gives the notes released to where the viewer reads from, and the
  same error as a channel that does not exist when nothing is (`nothing_released`).
- Numbers per agent and chain, never given twice; numbers only for the turn's own channel.
- Every read is recorded as knowledge; a read of private notes marks it private.

What the move means, found while looking at the code:

- **Lifecycle.** Notes are never deleted today: they rest in the agent's channel state while a
  channel is deleted, and go when the channel's state is purged - with its session
  (`procedure::session::remove_behind`, the only caller of `delete_channel_state`) - or when the
  agent is purged (`AgentManager`, `delete_agent_channel_states`). A document of the plugin has to
  go the same ways. `AgentMember` covers the agent. No mechanism covers the channel:
  `ScopeMember` asks for a named entity (`Scoped`) and deletes what lies in a scope in the
  caller's name, so that a channel's owner could not delete a channel in which a contributor's agent
  noted something (`ScopeNotEmpty`); `Unowned` is the kernel's. No entity lies `Within` a channel
  today.
- **Rights.** The board is read by whoever reads the channel, decided by the channel's view, not by
  the rights of a document - as the views read the agents' states today, from the state store. The
  plugin reads the documents for the board in the kernel's name, once the channel's view decided.
- **Who hears of a change.** `channel::AGENT_STATE_CHANGED` is announced against the channel, so
  that every reader of the channel reloads its board. The updates of a document reach its owner
  alone (as the memory's do), which would leave the "Notes" tab of everyone else stale. After the
  move `channel_state_changed` has no user left (`finish_turn` reports only notes;
  `actor/agent.rs`, `actor/commands.rs`), nor has `AGENT_STATE_CHANGED` but tests.
- **Reclassifying.** The kernel asks `Reclassified::Note { author, attributes }` of the access
  policy, which the ownership policy answers by `author == caller_id`. The memory asks a custom
  verb of its own (`MemoryAccess`, `RECLASSIFY`, `Requirement::Owner`). A `Requirement` that says
  "its author" does not exist (`Grant`, `Owner`).
- **`/channel notes`** is one of the commands of the kernel's component `channel`
  (`command/channel/mod.rs`). A plugin can register a component providing
  `CommandProvider<dyn ChannelCommand>`, but the global commands are configured
  (`AppConfig::commands`, by default `help`, `participant`, `agent` and `channel`), so its component
  would be in no session's commands until a configuration names it.
- **Atomic no longer.** Today the notes and the transcript are stored in one put of the channel
  state. Afterwards the plugin commits the notes as the turn completes, before `TurnFinished`; a
  commit refused or failed is lost and logged, as 518 accepted for the memory (its plan A).
- **The origin.** `NoteOrigin::transcript` is written (`finish_turn`) and read nowhere; it is
  knowledge of the agent side, which a plugin does not take.
- **Branches.** The chains do not branch with the message tree: `finish_turn` records the message a
  change follows "what branching (issue 08) will find the changes by", nothing reads it so.
  606 states that the notes of today branch; they do not yet.
- **Texts.** The texts are the agents' (`agents.notes`, `agents.private_notes`,
  `agents.tools.notes.…`, `agents.tools.private_notes.…`); in a namespace of the crate they change
  their keys. No configured variant replaces them (`examples/prompts/gemma4` replaces
  `results.denied` alone).
- **Uses elsewhere.** `caelumora-memory` shows its tool calls by `NOTE_PRESENTATION` of the agents'
  `notes` component; the web UI knows the presentations `note` and `note_list` by name. The domain's
  ledger tests use `NoteChain`. The seed's bundle `agent-basics` and the personality
  `sassy-assistant` name the kinds `notes` and `private_notes`, whose names stay.

## The plan

The pattern followed is `caelumora-memory`
([518](518-memory-is-reached-through-the-agents-state.md),
[PLUGINS.md](../docs/PLUGINS.md#what-a-system-service-offers-an-agents-work)): one crate,
`caelumora-notes`, cut by features into the chains and the document (no feature), the faces (`api`),
the plugin (`kernel`, and `server` for the routes, as `caelumora-skills` and `caelumora-artifacts`
have) and the library (`library`). Public and private notes move together: they share every
mechanism (`NoteKind`), and moving one first would keep `AgentNotes` in two crates for a step.

### Where it lives

| Part | Side | Crate, feature |
|---|---|---|
| `NoteId`, `Note`, `NoteChain`, `NoteKind`, `NoteChange`, the document `AgentNotes`, `PublishedNotes`, `PrivateNotesElsewhere` | both read it | `caelumora-notes`, no feature |
| `NotesReader`, `NotesWriter`, `NotesError` | the API of the service | `caelumora-notes`, `api` |
| `NotesManager`, the views, the reader's and the writer's binding, the plugin, the question of reclassifying, `/channel notes` | the kernel's | `caelumora-notes`, `kernel` |
| the routes of the board and of the private notes | the kernel's | `caelumora-notes`, `server` |
| `TurnNotes` (today `AgentNotes` of the agents), the components `notes` and `private_notes`, `/private-notes @agent`, the texts | the agent side | `caelumora-notes`, `library` |

### The interfaces

Proposed, to be agreed before anything is built; one doc line each.

```rust
// No feature.

/// Which of an agent's two chains in a channel: published, or read by the agent alone.
pub enum NoteKind { Public, Private }

/// A change to one of the agent's chains in the work's channel, as a turn stages it; where it was
/// made the plugin derives from the place of the work.
pub struct NoteChange { pub kind: NoteKind, pub change: Change<NoteId> }
impl StagedChange for NoteChange { const KIND: &'static str = "notes"; }

/// The notes of one agent in one channel, as they are kept: its public and its private chain.
pub struct AgentNotes {
    pub id: AgentNotesId,          // made of the agent and the channel (`AgentNotesId::of`)
    pub agent: AgentId,
    pub channel: ChannelId,
    pub public: NoteChain,
    pub private: NoteChain,
    pub security_attributes: SecurityAttributes, // owned by the agent's owner
    // deleted, revision, purging - as every document
}

// `api`.

/// What a work reads of notes: its agent's own in its channel, and what it may read elsewhere.
#[async_trait]
pub trait NotesReader: Send + Sync {
    /// The agent's notes of `kind` in the work's channel as they stand, with the chain's digest.
    fn own(&self, kind: NoteKind) -> (&[Note], Option<Digest>);

    /// What the agents taking part in `channel` of the work's session published, as far as the
    /// work may read it - the same error as for no such channel where nothing is.
    async fn published_in(&self, channel: ChannelId) -> Result<Vec<PublishedNotes>, NotesError>;

    /// The agent's private notes in the session's other channels, as far as each may be copied
    /// into the work's place.
    async fn own_private_elsewhere(&self) -> Vec<PrivateNotesElsewhere>;
}

/// What a turn changes of its agent's notes in the work's channel, committed when it completes.
#[async_trait]
pub trait NotesWriter: Committing<NoteChange> {
    /// Whether a note may be reclassified as `classification` here: asked before the turn stages
    /// it, and again as it is committed.
    async fn may_reclassify(&self, classification: &Classification) -> bool;
}

// `kernel`.

/// What a channel's view lists of notes: the board.
#[async_trait]
pub trait ChannelNotes {
    /// The notes the agents taking part published, as far as the viewer reads the channel.
    async fn notes(&self) -> Result<Vec<PublishedNotes>, NotesError>;
}

/// What a session's view lists of an agent's private notes, for whoever may read them.
#[async_trait]
pub trait SessionNotes {
    /// The private notes `agent` wrote in the session's channels, of each channel the viewer reads.
    async fn private_notes_of(&self, agent: AgentId) -> Result<Vec<PrivateNotesIn>, NotesError>;
}
```

`ChannelNotes` is an extension trait on `ChannelView`, `SessionNotes` on `SessionView`, as the
artifacts list theirs. The reader is made per work by the plugin, from the work's `SessionView`;
`own` is a snapshot taken then, as `MemoryReader::recalled` is. Numbers stay as they are given now:
derived when a turn stages, from `own` and the turn's staged changes - one actor per agent and
channel writes its chains; a number reserved by the plugin, as the memory's, comes with the chains
several write, in 606. `KnowledgeSource::Notes` and `PrivateNotes` become `Entity` with the kinds
of the crate, as `KnowledgeSource::Memory` did; the REST answer goes on saying `source: "notes"`
and `"private_notes"`. `NoteOrigin::transcript` goes; the origin of a change is the message it
follows, which the commit is given (`after`).

### What goes

- From the domain: `note.rs`, `AgentChannelState::notes` and `private_notes`, `TurnState::notes`
  and `private_notes`, the notes of `finish_turn` and its answer, `KnowledgeSource::Notes` and
  `PrivateNotes`, `PublishedNotes`, `Detail::NOTES`, `ChannelParticipantSnapshot::notes`,
  `channel::AGENT_STATE_CHANGED`. The ledger's tests get a kind of ledger of their own.
- From the API: `ChannelReader::notes`, `SessionReader::own_private_notes`,
  `PrivateNotesElsewhere`, `ActorRuntime::may_reclassify_note`,
  `ActorRuntime::announce_channel_state_changed`.
- From the kernel: the notes of `ChannelView`, `ParticipantView` and `SessionView`,
  `command/channel/notes.rs`, `may_reclassify_note` of the agent's runtime.
- From the agents: `notes.rs`, `components/notes.rs`, `components/private_notes.rs`,
  `Turn::stage_note`, `staged_notes`, `may_reclassify_note`, `CommittedTurn::channel_state_changed`.
- From REST: the two routes and their types move to the plugin's `server` feature;
  `ChannelResponse::notes` goes.

### The steps

Each step is green and changes no behaviour; each is reviewed and committed on its own.

1. **The snapshot no longer carries notes.** `Detail::NOTES`, `ChannelParticipantSnapshot::notes`
   and `ChannelResponse::notes` go; the web UI loads the board with `GET …/channels/{cid}/notes`
   when it opens a channel, as it reloads it on a change. Done in place, before anything moves.
2. **The kernel offers what a plugin needs.** A way to remove what a plugin keeps behind a
   channel when the channel's state is purged (open question 1); `ChannelView::nothing_released`
   reachable by an extension; the way a plugin's command reaches the sessions' commands (open
   question 3).
3. **The notes move into `caelumora-notes`.** The document, the manager, the faces and their
   binding, the plugin with its views, routes, command and announcement, the library with the
   components; everything listed under *What goes*. The programs list `NotesPlugin` -
   `caelumora-app` in `rest_plugins()` - and `NotesLibrary`, as the memory's; every test world whose
   personalities use `notes` or `private_notes` names both. `scripts/library-deps.sh` and
   `scripts/crate-graph.sh` know the crate. The tests move with what they test: the board's and
   581's (`caelumora-kernel/src/view/mod.rs`), the security suites' (`caelumora-kernel/tests` and
   `caelumora-agents/tests`), `/channel notes` (`channel/tests/commands.rs`), the agents' channel
   tests of notes, the components' own. The documents: MEMORY.md, PLUGINS.md, ARCHITECTURE.md,
   CRATES.md, AGENT-TURN.md (one way to stage), CHANNELS.md (581's reasons), COMMANDS.md, WEB-UI.md,
   CONFIDENTIALITY.md (reclassifying), SECURITY.md and STORAGE.md where they name the notes.

Stored data to delete with step 3: every agent's channel state; and the histories, transcripts and
channel states of every channel in which an agent read notes or private notes, whose records name
the removed variants of `KnowledgeSource` - in practice every channel with an agent of `notes` or
`private_notes`. No compatibility code
([CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#crates-and-interfaces)).

### Open questions

1. **What removes the notes with a channel.** (a) A trait of its own beside `AgentMember`,
   `ChannelMember::remove_behind(channel)`, called where the channel's state is purged; (b)
   `AgentMember` generalised to what is kept behind something, for the agent and the channel alike;
   (c) the document as a `ScopeMember` of the channel's scope - which would delete it with the
   channel in the caller's name, refuse a channel's deletion where a contributor's agent noted
   something, and needs a name. Recommended: (b) if the two read alike, otherwise (a); not (c).
   Whichever is chosen is what 606's channel notes go with too.
2. **How the document is cut.** (a) One per agent and channel, as the chains lie today: one writer,
   a board of as many reads as today, the agent's purge finds its documents by a field, the
   channel's by another; (b) one per channel, every agent's chains in it: one read for the board,
   but several writers under a lock, and an agent's purge changes every channel it noted in; (c) one
   per agent, as the memory: every channel's purge changes the documents of its agents.
   Recommended: (a); 606's notes of the channel then are a document per channel of their own.
3. **How `/channel notes` reaches a session.** (a) The plugin registers a component of its own
   (`notes`) providing the command, and the programs add it to their configured commands, as the
   terminal adds `workspace` - a server whose configuration lists the commands loses it until it
   names it; (b) a plugin contributes to the default commands, a mechanism of the kernel's that does
   not exist; (c) the command stays the kernel's and reads the board through `ChannelNotes` - which
   the kernel cannot, as it does not depend on the plugin. Recommended: (a).
4. **Who may reclassify.** (a) The plugin asks `Reclassified::Note` as the kernel asks it today -
   no behaviour changes, but `caelumora-security` keeps a word for a plugin's subject; (b) a
   custom verb of the plugin, as the memory's - which needs a `Requirement` that names the author,
   a change of the policies. Recommended: (a) in this issue, (b) as a question of its own.
5. **Who hears of a change.** The plugin announces a custom event of the channel's
   (`notes_changed`, without content) against the channel, in the place of
   `channel::AGENT_STATE_CHANGED`, which goes with `channel_state_changed`; the web UI reloads the
   board on it. Besides, the document's own updates go to its owner, as every writer of a document
   announces. Recommended as described.

## Done when

- The open questions are decided, the interfaces agreed.
- No crate but `caelumora-notes` knows notes: the domain, the API, the kernel, the security policy
  and the agents' library name none of its types, and a turn stages in one way.
- The behaviour of notes and private notes is unchanged - held by the tests that moved with them.
- MEMORY.md, PLUGINS.md and the documents listed in step 3 describe the notes as the system service
  of `caelumora-notes`.

## History

**2026-11-25** - Opened at the maintainer's request while looking at
[606](606-no-notes-belong-to-the-channel.md): whether the notes that belong to the agents could move
into a crate of their own, as a plugin and a library, before the notes of the channel are built
beside them. The plan above, with its open questions, was worked out with an advisor and awaits the
maintainer's decision.
