# 16 — Context compression

**Status:** in progress
**Area:** Context & knowledge

## The problem

`MessageHistory` grows for as long as a session lives, and the context providers assemble a
`MessageContext` from it on every turn. Nothing bounds either. A long-running session eventually
exceeds the model's context window, and there is no strategy for what to drop or how to summarise
before it does.

## Why it matters

This is the hard limit on session length. Everything else about a long-running agent — memory,
knowledge sharing, a session that spans days — presumes an answer to this one.

It is also a cost issue well before it is a correctness issue: an unbounded context is re-sent on
every turn.

## Considerations

- The mechanism most likely already exists in the right shape. Context is assembled by components
  implementing `ContextProvider` / `PlanPreProcessor`, so compression is plausibly a component in
  that chain rather than a new subsystem — consistent with how the architecture handles composition
  elsewhere.
- The strategies differ in what they destroy:
  - **Truncation** — drop the oldest. Cheap, predictable, loses the beginning of the conversation,
    which is often where the task was defined.
  - **Summarisation** — replace old turns with a model-generated summary. Preserves gist, costs a
    model call, and introduces a lossy artifact that subsequent turns cannot distinguish from ground
    truth.
  - **Retrieval** — keep everything, select what is relevant per turn. Most capable, most machinery,
    and closest to [18](18-knowledge-sharing.md).
- Compression must not mutate the stored history — the record and the working context are different
  things. See [06](06-context-in-message-for-auditability.md), which needs the *uncompressed* truth
  to remain reachable.
- Tool results are frequently the bulk of a context and the least useful to keep verbatim; they may
  warrant their own rule.

## Done when

- A session can run past the model's context window without failing.
- The chosen strategy is pluggable per agent rather than hard-coded.
- A user can compact an agent's context by hand with the command `/compact`.

## Decision

Decided 2026-10-10 with the maintainer. Compaction is built from what the transcript already is:

1.  **A compacted context is a new branch of the same `Transcript`.** The head goes back to the
    root (`Chain::move_head(None)`, as `Transcript::start_anew` does for an external agent), and
    the new branch begins with what replaces the oldest messages - a summary - followed by what is
    kept verbatim, copied as it stands. The old branch is not touched: every `ContextRef` recorded
    before resolves through `Chain::prefix` as it did, and `ContextResolver` does not change.
2.  **"C′ came from C" is an entry of the agent's history**, `HistoryEntry::Compacted { from, to,
    .. }`: the history records what happened to the agent, it is outside every digest of the
    transcript, and the history panel shows it in its place.
3.  **What was given to the agent to read is an entry of its own**, `TranscriptEntry::Given {
    message, source }`: a skill loaded for it (issue 22), hashed as the message is - the model is
    given the same - with the knowledge it is beside, outside the digest. Without it nothing in the
    transcript tells a loaded skill from a message, and a compaction could not keep it.
4.  **One component, `compaction`**, holds everything about compaction, as `skills` holds
    everything about skills: `/compact` now (an `AgentCommandProvider`, `owners_only`), the
    trigger of [284](284-automatic-context-compaction.md) later. The strategy is the component's;
    the chain mechanics are the state's (`AgentChannelState::compact`).
5.  **A loaded skill is loaded again**, after the summary, as it stands now - which is what the
    agent would be given on its next load anyway (SKILLS.md: "a skill changed afterwards is
    loaded anew next time"). Keeping its text verbatim instead was considered: for a skill given
    with `/skill` the text is at hand (`Given`), for one the model loaded with `load_skill` it is
    in a tool result that nothing marks as a load - recognising it would couple the compaction to
    the skills component's tool. One rule, then: the skills loaded in the summarised stretch -
    known from the `Given` entries and from the `KnowledgeSource::Skill` the history recorded with
    the agent's messages - are loaded again, each as a `Given`.
6.  **Not here:** compacting on the agent's own, the threshold, telling the channel of an
    automatic compaction ([284](284-automatic-context-compaction.md)); retrieval
    ([18](18-knowledge-sharing.md)).

Considered and not chosen - **a cut recorded in the `ContextRef`** instead of a branch (`since:
Digest` and the summary as a prompt part, as the profile of issue 63 is recorded): less structure,
but the transcript's digest would no longer name what the model was given, `ContextRef` and
`begin_turn` would have to carry a compaction state from turn to turn, and 06 already decided for a
new context C′.

## Plan

Written against the code as of `562e697`. Three steps, each reviewed on its own: a refactoring that
makes what was given recognisable, the mechanism in the model with its record, then the component
and its command.

### A. What the code shows

- **The chain is a tree** (`caelumora_model::chain`, issue 08): `push_with` appends after the head
  and dedupes an entry whose digest is already there (same predecessor, same content - the head
  moves to it); `move_head(None)` goes back to the root, what follows stays as a branch; `prefix`
  finds an entry on any branch; `all_entries` walks every branch. `Transcript::start_anew` is
  `move_head(None)`, used by the external agent before every call (`actor/external_agent.rs`).
- **What the model is given** is resolved, not stored (`ContextResolver::resolve`): the prefix a
  `ContextRef::transcript` names, `as_they_stand` (modifications, revocations and
  reclassifications folded in), lowered turns reduced to `names_only`, each message as the
  profile gives it, the prompt parts after the entries they were placed after.
- **`AgentChannelState`** holds one `Transcript` and one `MessageHistory` per agent and channel.
  `begin_turn` takes `transcript.digest()` into the turn's context and `see`s every message of the
  active path; `said` and `given` push to the transcript and the history at once and move the
  turn's context on; `finish_turn` records `CommandExecuted` and `TurnFinished`; `usage` sums the
  tokens of the agent's own messages over `all_entries`; `recorded_context_of` finds a message's
  `ContextRef` in the history.
- **A given skill** (`component_impl/skills.rs`, `/skill`) is a `User` message in the issuer's
  name whose content is `[Skill: <name>]` and the body, pushed with `Turn::give` after
  `Turn::record_knowledge` - two calls; nothing on the entry says it is a skill. `load_skill`
  answers the skill as a tool result (`ToolPresentation::Skill`, which `create_skill` and
  `update_skill` carry as well); the turn records `KnowledgeSource::Skill { skill, digest }`,
  which the history keeps in the `ContextRef` of what the turn says afterwards.
- **A command turn** (`actor/commands.rs::AgentCommands::carry_out`) begins with
  `begin_command_turn` - no tools, no `ContextProvider` -, hands the component a `Turn` with views,
  commits on `Ok` (the command recorded before `TurnFinished`, the state stored, the answer sent
  as `CommandExecuted`, channel-wide unless private) and on `Err` drops the turn with
  `state.turn = None` - but what was pushed to the transcript meanwhile stays, so a component
  changes the chain only once nothing can fail. `AgentCommands` has the profile, not the
  `LLMClient`; a component gets the agent's model with `context.one::<dyn LLMClient>()` in a
  `register_async` factory (a facet of which there is exactly one, `llm::register_all`).
- **The record reaches the UI** as `HistoryItemResponse` (`caelumora-rest-server/src/models.rs`,
  a variant per `HistoryEntry` the items show) and `HistoryItem<M>` in the web UI
  (`types/index.ts`), read by `chatMessages` (`messages.ts`) for the chat and `rowsOf`
  (`components/HistoryPanel.tsx`) for the agent's history panel. The chat shows a command carried
  out as `commandNotice`; the context panel (`ContextPanel`) shows the items of a
  `ResolvedContext`.

### B. Step 1 - what was given is recognisable (`TranscriptEntry::Given`)

A refactoring without a change in behaviour, so that the compaction can tell a loaded skill from a
message.

- **`caelumora_model::transcript::TranscriptEntry::Given { message: Message, source:
  KnowledgeSource }`**: a message the agent was given to read in a turn - not said by it, not heard
  from another - and what knowledge it is. `message()` answers it as it answers a `Message`, so
  `as_they_stand`, `messages`, `of_turn`, `Transcript::message` and `in_transcript` read it without
  a change; `changed()` and `ended_turn()` answer `None`; `classification()` the message's.
- **Its digest is `message_chain`**, the same a `Message` entry gets: the model is given the same
  thing, and two identical contexts get the same digest however they came about (06). The source
  stays outside the digest, like `turn_id`. `digest::transcript_chain` maps the variant.
- **`AgentChannelState::given(message, knowledge: KnowledgeRef)`** pushes `Given { message,
  source: knowledge.source }` and records the knowledge on the turn under way (today the caller
  does that in a call of its own); the history keeps `Said { message, context: None }` as today.
  `Turn::give(message, knowledge)` likewise; `skills.rs` makes one call where it made two.
- **Tests:** a `Given` entry is read as its message; its digest equals the message's as a `Message`
  entry; `given` records the knowledge on the turn. Existing tests of `given` adjusted.
- **Docs:** AGENT-TURN.md (*The transcript as a chain*, the paragraph on a message given),
  SKILLS.md (*How a skill reaches the model*).
- **Stored data:** a skill given before this step is stored as a `Message` entry and stays one - a
  compaction will not know it as a skill. No reader of the old form; the local
  `agent_channel_state` data is to be deleted, or the difference accepted.

### C. Step 2 - the mechanism and its record

In the model, with the REST and web UI parts that show the record.

- **`HistoryEntry::Compacted { turn_id: TurnId, actor: ActorId, from: Digest, to: Digest, at }`**:
  in the turn `turn_id`, `actor` replaced its context `from` - the head of the transcript before -
  by `to`, the head of the new branch. Digest scheme `caelumora.history-compacted.v1` over the
  predecessor, the turn, the actor and both digests - not the time, as the other ends and marks.
  `MessageHistory::record_compaction` beside `record_command`; `items` passes it through like
  `CommandExecuted`. Recorded in the agent's history only: the channel knows of a `/compact`
  through `CommandExecuted`, and of an automatic compaction through 284's event.
- **`AgentChannelState::compact(&mut self, actor: ActorId, replacement: Vec<TranscriptEntry>,
  keep_from: Option<Digest>) -> Option<(Digest, Digest)>`**, the one rule of the chain mechanics;
  the state does not know what a summary is:
  1.  `keep_from` names the first entry of the active path that is kept, `None` keeps nothing.
      Refused (`None`, nothing changes) if the transcript has no head or `keep_from` is not on the
      active path.
  2.  The kept stretch is read `as_they_stand` from `keep_from` to the head: a message that stands
      becomes a `Message` - a `Given` stays a `Given` with its source -, one taken back is left
      out, `TurnFinished` and `TurnGivenUp` are copied, `Modified`, `Revoked` and `Reclassified`
      are not (they are folded into what stands). What a kept message said before the cut about
      a message in the summarised stretch - a modification of it - is gone with it.
  3.  `from = transcript.digest()`; `start_anew()`; `replacement` pushed in order, then the kept
      entries. Every copy gets a new digest, since its predecessor is another; one that happens to
      exist already is that entry (`push_with`), which is right: the same context.
  4.  `to = transcript.digest()`; the history records `Compacted { from, to, .. }`, then every
      message of `replacement` as given (`Said { context: None }`), after it. The kept messages
      are in the history already, under the same ids.
  5.  The turn under way sees the replacement (`Work::see_message`) and its context moves to
      `to`. Between turns it is refused: a compaction is a turn's, as what is said is.
- **`AgentChannelState::usage`** counts every message once by id: a kept message is copied with
  its tokens, and a copy is not a call.
- **`Turn::compact(replacement, keep_from)`**, `pub`, for components: calls the state and records
  the knowledge of every `Given` in `replacement` on the turn (`record_knowledge`, with the
  message's classification), so that the context of what the turn says afterwards names it.
- **REST:** `HistoryItemResponse::Compacted { turn_id, actor, from, to, at }`; `of` maps it.
- **Web UI:** `HistoryItem<M>` gets `compacted`; `rowsOf` a row `{kind: 'compacted', turnId,
  at}` rendered as a mark like a turn given up - "compacted its context" and when; `chatMessages`
  shows nothing for it (the chat has the command's notice). The summary and the skills given again
  appear below it as messages without a context, as a given skill does today.
- **Tests (model):** after a compaction the active path is summary, given skills and the kept
  stretch as it stood, every `ContextRef` recorded before resolves to what it resolved to, the
  context recorded with the next message said resolves to the new branch; a modification in the
  kept stretch is folded, a revoked message left out, a turn's end copied; `usage` counts a copied
  message once; `Compacted` is among `items`; refused between turns and off the active path;
  `retried` of a summarised message answers `NotItsAnswer`, of a kept one works.
- **Docs:** AGENT-TURN.md - *The context only grows* becomes *Compaction* (the branch, the record,
  what the resolver needs of it: nothing), and the stale sentence that a turn's messages are
  appended only when it is taken over is corrected (issue 260); WEB-UI.md (*What an agent saw*,
  the history panel's mark).

### D. Step 3 - the component and `/compact`

- **`component_impl/compaction.rs`**, kind `compaction`, facet `AgentCommandProvider`, registered
  with `register_async`: the factory takes `context.one::<dyn LLMClient>()`, the agent's own
  model. A container without one - an external agent - fails to build it with `InvalidConfig`
  ("compaction needs a model"); nothing here can compact a remote agent's context.
- **Parameters:** `keep_last` (messages kept verbatim, default `10`). The cut is moved back to the
  first message of the turn the `keep_last`-th last message belongs to (`Message::turn_id`), so
  that no tool call is parted from its result and no turn's end from its messages; with no turn
  id, the message itself. The summariser's instruction is a constant of the component (issue 57
  applies to it as to every prompt).
- **`/compact @agent`** - `Command { name: "compact", usage: None, owners_only: true }`: compacting
  a shared agent's transcript is like approving its calls (269). `execute`:
  1.  Reads the active path as it stands (`turn.agent_channel_state().transcript`) and chooses
      the cut. Nothing before it - fewer messages than `keep_last` - is refused with
      `InvalidArguments`: "nothing to compact".
  2.  Collects the skills loaded in the summarised stretch: the `Given { source: Skill }` entries,
      and the `KnowledgeSource::Skill` in the `ContextRef` the history recorded with each of the
      agent's own messages there (`recorded_context_of`); one skill once. Each is read anew
      through the turn's views - `usable_skill(id)` beside `usable_skill_named`, the same check
      (`UseSkill`), one no longer usable left out and named in the answer - and becomes a `Given`
      with the skill's classification and knowledge, its text as `/skill` words it (`loaded`,
      shared with `skills.rs` - moved to the model or to a function both use).
  3.  Builds the summariser's input as the model would be given the stretch: a `ContextRef {
      transcript: <digest of the last summarised entry>, profile: <the turn's>, .. }` with no
      prompts and no tools, resolved with `ContextResolver::of(state)` for the turn - so lowered
      turns, reasoning and sender markers follow the same rules as in every call -, followed by
      the instruction as a `user` message: summarise what matters for going on - the task, what
      was decided, what was done and with which files, what is open -, name no skill's text.
      `count_model_call`; `llm_client.chat_completion(&input, &[])`; the `Complete` messages'
      content joined. A model that answers nothing, or an error: `CommandError::Failed`, and
      nothing has changed.
  4.  The summary: `Message { role: User, sender: the agent, content: titled("Summary of the
      conversation so far", text), classification: turn.label(), .. }` - the label is the join of
      everything the transcript holds, which `begin_turn` saw -, with the tokens of its call, as a
      message the model produced carries them (`usage` then shows the summariser's call as the
      last). Nobody is marked as its sender: it is the agent's own.
  5.  `turn.compact(replacement, keep_from)` - the summary, then the skills -, which cannot fail
      any more; then the answer, channel-wide: "@bob compacted its context: N messages summarised,
      M kept, skills loaded again: a, b" (and which skill could not be).
- **Config and seed:** `compaction` in `agent-basics` of `examples/seed.yaml`, so that every
  personality of the example has it (issue 270).
- **Security suite:** a test that `/compact` of a shared agent is its owners' alone - bob, a
  contributor of alice's channel, is refused, alice is not - with a silent model kind, as
  `with_a_commanded_agent` does for the test provider.
- **Tests (runtime):** the component with a stub `LLMClient`: the cut at a turn boundary, a skill
  given with `/skill` and one loaded by tool both given again, the refusal of a short transcript,
  the answer's wording; the resolver sees the new branch in the next turn.
- **Docs:** TOOLS-AND-COMMANDS.md (the commands table, *Commands of an agent*), COMPONENTS.md (the
  components table), SKILLS.md (*Not decided yet*: decided - loaded again), AGENT-TURN.md
  (*Compaction*: the strategy is the component's), CONFIGURATION.md if the parameter is named
  there, PLAN.md (B2: 16 done, 284 placed); this issue's status and history.

### E. What to watch

- **The prompt cache** is lost with the new branch, once; expected.
- **A retry** (issues 08, 234) reaches only the active path: a message in the summarised stretch
  can no longer be answered again (`NotItsAnswer`); a kept one can, under its id.
- **Notes** anchor at a transcript digest (`NoteOrigin`); the old branch keeps it resolvable.
- **The token gauge** (issue 209) shows the last call: after a `/compact` the summariser's call,
  with the size of the summarised stretch, until the agent's next call - and a command turn
  announces no `TurnFinished`, so the web UI fetches the member anew only on the next turn's end.
  Acceptable here; 284's event is where it gets better.
- **The command turn is not resumed** after a restart (269 as built): a `/compact` cut off during
  the model call did nothing and is asked for again; the chain changes only after the call.
- **`/compact @all`** compacts every agent that carries it out, one turn each.
- **What the summary may hold** is what the transcript held: classified as the turn's label, it
  flows where the transcript flowed; the instruction keeps skills out of it, and the skills are
  given again in full in any case.
- **Measuring** which `keep_last` and which instruction suit the default model is work beside the
  code (`work/`, LLM-MODELS.md), not a condition of this issue.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Constraint from [06](06-context-in-message-for-auditability.md), which made the
  context a hash chain the history refers to: compaction must not rewrite the existing context. It
  produces a new context from it; history entries written before keep referring to the old one,
  which must stay resolvable, and only messages produced afterwards refer to the new one. The new
  context's origin is recorded as a relation of its own (not the chain's predecessor link, not part
  of its digest). Details in 06's history.
- **2026-09-30** — From [63](63-reasoning-in-model-input.md), decided with the maintainer: what a
  model call is given differs from the transcript since 63 - reasoning the model's rule leaves out.
  It is not stored; the `ContextRef` records the rule and the turn's share of the transcript
  (`reasoning`, `turn_entries`) as data, and `resolve` derives the given input again. An open filter
  function on the `LLMClient` (`fn(Message) -> Option<Message>`, per model, for any adaptation) was
  considered and not done: a function cannot be recorded, so either the context panel would
  recompute with today's client (against "records are not recomputed"), or the given input would
  have to be stored - with a reference per differing entry in every `ContextRef`, quadratic in the
  transcript's length. Compression will have to store a context that is not the transcript anyway;
  once that mechanism exists, a filter function's result is one more such stored context, and the
  open function becomes cheap. Reconsider it here.
- **2026-10-05** — `/compact` moved here from [28](28-user-commands.md), which is done otherwise. It
  is the manual trigger for this issue's mechanism, not a mechanism of its own: a `CommandProvider`
  like `/help` and `/wakeup`, configured in `AppConfig::commands`, run by the user's actor and
  checked with `ExecuteCommand`. Its answer is channel-wide, since everyone in the channel should
  know that an agent's context was compacted. Like compression itself, it must not mutate the
  stored history. Which agent it compacts in a channel with several - `/compact [agent]` like
  `/wakeup` - and who may compact a shared agent ([14](14-approval-privilege.md)) are decided with
  it.
- **2026-10-09** — The shape of `/compact` changes with [269](269-agent-commands.md): a command the
  user's actor runs cannot change an agent's transcript, which only the agent's actor writes. It
  becomes a command *of the agent* - an `AgentCommandProvider` component of the personality
  (`compact_command`), sent to the agent as `CommandRequested`, carried out by it in a turn of its
  own and answered channel-wide with `CommandExecuted`. The agent is always named - `/compact
  @bob`, `/compact @all` -; who may compact a shared agent is the command's `owners_only`, decided
  by the ownership policy as approving its calls is; an external agent compacts only if its
  component has a word for it. 269 comes first; also: a skill loaded into the transcript (22) must
  survive a compaction.
- **2026-10-10** — Decided with the maintainer (see *Decision*): a compacted context is a new
  branch of the same transcript, its origin an entry of the agent's history, a loaded skill a
  `Given` entry and loaded again after a compaction, everything in one component `compaction`
  with `/compact`. Compacting on the agent's own - the first *Done when* above - is split off
  into [284](284-automatic-context-compaction.md); this issue builds the mechanism and the
  command. The plan is under *Plan*.
- **2026-10-10** — Step 1 done (plan B): `TranscriptEntry::Given { message, source }` records what
  an agent was given to read with the knowledge it is; `Transcript::give`,
  `AgentChannelState::given(message, knowledge)` and `Turn::give(message, knowledge)` take the
  knowledge with the message, so `/skill` makes one call where it made two. Hashed as the message,
  so every digest stays what it was. Stored data: a skill given before stays a `Message` entry;
  delete the local `agent_channel_state` data, or accept that a compaction will not know it.
- **2026-10-10** — Step 2 done (plan C): `AgentChannelState::compact` begins the transcript anew
  with the replacement - each message with the knowledge it is, if it is one, so a skill goes in as
  `Given` - and the entries kept as they stand; `HistoryEntry::Compacted { turn_id, actor, from, to,
  at }` records it in the agent's history, `Turn::compact` offers it to components. Prompt parts
  the turn placed after an entry are dropped with the old branch, so that a turn's context always
  resolves; who placed them places them again (284). `usage` counts a message kept once. REST
  gives the history item as `{"compacted": {…}}`, the web UI's history panel marks it. Beside it:
  REST-SERVER.md did not list `command_executed` among the history items; it does now.
