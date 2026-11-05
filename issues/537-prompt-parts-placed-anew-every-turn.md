# 537 — What an agent is given besides the conversation is placed anew in every turn

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** A prompt part before the turn's input is built anew in every turn and stands elsewhere in the next, so a model's prompt cache loses the whole turn before with every turn; only an external agent is told a part once and then only when it changed, and that in two ways, by its components and by its actor.

## The problem

**The cache loses the turn before.** A part at `BeforeTurnInput` - notes, private notes, memory,
the current time, when to mention - is placed before the newest message of the turn, and only in
that turn (`TurnContextProvider`, issue [47](47-prompt-parts-with-provenance.md)). The next turn
places its own before its own input; the part of the turn before is gone. A prompt cache - the
prefix cache of llama.cpp and Ollama as much as Anthropic's - reuses what a call has in common
with the call before up to the first token that differs, and that is where the part of the turn
before stood. So every turn has the whole turn before processed again: its input, every tool call
and result, its answer. In a chat that is little; in a turn that read ten files it is tens of
thousands of tokens, with every turn after it. Issue [424](424-time-in-the-prompt.md) took a part
before the input to serve the cache "since it moves with the input": that holds within a turn,
not from one to the next. A part at `Start` is built anew too, but breaks the cache only when its
text changes.

**Told where it changed, twice and for external agents alone.** An external agent's remote agent
keeps its conversation, so it is told a part once and then only when it changed - by two
mechanisms:

- the components `channel_participants`, `channel_briefing` and `channel_language` with
  `changes_only` remember what they told, in memory, per actor, and answer nothing while it is the
  same; after a restart they tell it again. The briefing tells that it was withdrawn when it
  becomes empty (`channel_briefing.rs`, `WITHDRAWN`);
- the external agent's actor gives only a part whose digest is not what it gave last of the
  part's component (`RemoteState::given_prompts`, stored with its state).

The components' memory does what the actor's does already, except the withdrawal, and is lost on a
restart where the actor's is not. An agent run here has neither: it is told every part in every
turn.

**For comparison.** Claude Code writes what its harness gives the model besides the conversation
into the chain of its session where it was given - the environment, the date, the instruction
files and the memory, the tools and skills to be had - and later only what changed (a date that
changed, `deferred_tools_delta`, `skill_listing`), after the user's input and framed as system
reminders; after a compaction it gives again what is still needed (`invoked_skills`). What its
model is given so grows only at its end.

## Why it matters

What a turn costs grows with the turn before it, though nothing in that turn changed. A local
model spends the time of processing the prompt again; a remote one, the writing of its cache.
Agents that work with tools - the workspace, a plan of several steps - are hit hardest.

Beside the cost: "given where it changed" is one concept with two implementations and no name, and
only one kind of agent has it. Made one mechanism, it is the same for both kinds of agent, and the
transcript records what was given where as it records a skill - a part given so is in the
transcript, not reconstructed from the turn's frame.

## Considerations

**A third position, chosen per component by the personality.** The personality already places
every prompt component (`position`); the new position places its part into the transcript:

```rust
pub enum PromptPosition {
    /// Before everything, as the system prompt, built anew every turn.
    Start,
    /// Transiently before the newest message of the turn, built anew every turn.
    BeforeTurnInput,
    /// Into the transcript as a `Given` entry of its component, where its text differs from
    /// what the component's last such entry on the active branch says.
    Given,
}
```

`Given` suits what changes rarely and long turns with tools on a model with a prompt cache;
`BeforeTurnInput` what changes every turn anyway, or what has to be judged anew under the rights of
every turn (see *Confidentiality* below). A changed personality takes effect at once either way: a
part whose text changed is given at the next call.

**The entry names its component.** `TranscriptEntry::Given { message, source }` says today what
knowledge a message given is, if any - a skill -, or nothing - a summary. A part is no knowledge, so
the entry has three states, and a type for them (*States are types*):

```rust
/// What a message given to the agent is.
pub enum GivenSource {
    /// Knowledge: a skill loaded, a file read (issue 323).
    Knowledge(KnowledgeSource),
    /// The part of the prompt component `component`, by its id (`PromptPart::source`).
    Part(String),
}
```

`Given { message, source: Option<GivenSource> }`; `None` stays a summary's. The knowledge a part
holds (`Prompt::knowledge`) is recorded with the turn that gives it, as now.

**One rule, at one place.** At the start of `Planning`, before every call of the model - where
`take_in_during_turn` hears what was said, once every call of the step before has its result, so
that nothing comes between a call and its result -, the turn gives every part at `Given` whose
component has no entry on the active branch with the part's digest (`Turn::give`). That one rule
covers the first turn, a part that changed, a compaction - the new branch holds no entry of the
component, so the part is given again after the summary without a `KnowledgeRestorer`, also in the
middle of a turn - and a turn resumed. The parts are assembled once, as the turn begins, as now;
the turn keeps those at `Given` with its working state, by digest, their texts with the prompt
texts.

**Withdrawn.** A component whose part stood on the active branch and gives none now - its text is
empty, or the personality no longer has it - is told as withdrawn, by the same rule: what stands
earlier in the transcript would otherwise hold. Two ways to word it:

- a text of the agents' catalogue, naming the part by its title (every part the agents write
  begins with its title in brackets, AGENT-TURN.md *Prompt parts*) - general, but it reads the
  title out of a text by a convention;
- the provider words its withdrawal itself, as the briefing does now - exact, but every provider
  given at `Given` needs a text for it.

**Where in the transcript.** A chain is only appended to, and the turn's input is in it before the
turn begins, so a part given at the turn's start follows the input. Claude Code places its context
so, framed as from the system. Issue 424 feared that a message after the input draws the answer to
it rather than the question; a part with its title, given as a system message where the model keeps
one (`ModelProfile::system_messages`, as the channel's notices are), is framed as no participant's.
To have it before the input, the turn would have to assemble its parts before it takes in the
request that begins it - before the turn and its situation exist.

**Confidentiality.** A part at `Given` is classified as its provider says, like every message the
turn is given, and counts for later turns as the transcript does; one of a turn given up as
`Lowered` goes as that turn's messages go. What it does not do is disappear: what a part no longer
holds - an entry of memory no longer recalled since its session became private again - stands
earlier in the transcript until a compaction, where a part at `BeforeTurnInput` is gone with the
next turn. That is the reason to keep such parts at `BeforeTurnInput`, and the documents say so.

**External agents.** The same position means the same for an external agent; what stands on the
remote side is recorded by `RemoteState::given_prompts`, since its transcript starts anew with every
call. `changes_only` and the components' memory of what they told go, the withdrawal becomes the
actor's by the rule above, and the seed's `hermes-remote` names `position: given` where it named
`changes_only`. Whether its actor then needs the remote state beside the transcript at all is a
question of its own.

**Not part of it.** What the `ContextResolver` words itself when it resolves an old context - the
senders' markers, the times, the notices - with the texts of the catalogue as they are now, so that
what an agent saw is not told exactly once a text changed: a question of its own, later.

## Steps

1. **Told where it changed, by the actor alone** (external agents): the withdrawal moves to the
   actor, `changes_only` and the components' memory go. Changes behaviour only after a restart,
   where a remote agent is no longer told again what it was told.
2. **`GivenSource`**: the entry of a message given names what it is. No change of behaviour.
3. **`PromptPosition::Given`** for agents run here: the rule before every call, the withdrawal, the
   role; for external agents, the position in place of `changes_only`. AGENT-TURN.md (*Prompt
   parts*, *Compaction*), COMPONENTS.md (the parameters of the prompt components), the seed.

## Done when

- A personality can place every prompt component at `Start`, `BeforeTurnInput` or `Given`, the
  same for agents run here and external agents, and nothing else says how often a part is told.
- A part at `Given` is in the transcript where its text changed, given again after a compaction
  and told as withdrawn when its component gives none any more; a turn without a change gives
  nothing.
- With every prompt part at `Start` or `Given`, what the model of an agent run here is given in a
  turn begins with all it was given in the turn before.

## History

- **2026-11-05** — Opened from a comparison with the session logs of Claude Code, whose harness
  writes what it gives its model into the session's chain where it changed. Measuring what the
  cache loses was skipped by decision; exactly reconstructing what an agent saw is put after it.
- **2026-11-05** — Decided with the maintainer: a provider words its withdrawal itself
  (`PromptProvider::withdrawn`, the second of the two ways above), and for an agent run here a part
  at `Given` that changed or holds nothing takes its entry before back (`TranscriptEntry::Revoked`)
  rather than leaving it to stand: the model never reads what no longer holds, a withdrawal needs no
  text there, and a part that no longer holds what may not be recalled any more is gone at once. The
  cache is lost where a part changes, from where it stood, and only then.
- **2026-11-05** — Step 1 done. Unlike *External agents* above says, the withdrawal is not
  the actor's: whether a component's part is empty only its context provider sees, so
  `ExternalContextProvider` gives the withdrawal of a component whose part the remote agent was
  given (`RemoteState::has_given_any`) and that has none now, and the actor gives it, as every
  part, only where it is not what it gave last. The actor drops a part a failed send left pending
  once its component gives none. `changes_only`, the components' memory of what they told and
  `ChannelPartParams` are gone. `withdrawn` takes no situation, since neither withdrawal reads one.
