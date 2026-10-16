# 389 — A compaction reads from the history what it gives the model again

**Status:** done
**Area:** Context and knowledge

## The problem

A compaction gives the agent again what it had read in the part it summarises - a skill, any entity
of a kind a `KnowledgeRestorer` serves - unless it is still there in the part kept
(`SummaryCompaction`, issue [284](284-automatic-context-compaction.md)). What was given is read
from two places:

- what was given as a message, from the transcript: `TranscriptEntry::Given { source }`;
- what a turn read otherwise - recorded as knowledge with what it said afterwards (`load_skill`,
  say, through `Turn::record_knowledge`) - from the history: `given_read` collects the
  `KnowledgeSource::Entity` of every `ContextRef` the history recorded with a message.

The second reads the history to build what the model is given. The history of an agent is the
record of what was said and what it was produced from, for people to read; what the model is given
is built from the transcript alone. The transcript does not hold what a turn read through a tool or
a prompt part, so the compaction has nowhere else to find it.

## Why it matters

- It breaks the rule that the model's input is built from the transcript alone: the history is
  informational and may be held only in part
  ([387](387-agent-history-window-without-compaction.md)). A history held in part would give again
  less than was read, without anyone noticing.
- What was read is a property of the transcript's branch - it is what the model was shown there -
  and is found today only by going through a second chain by message id.

## Considerations

- **Where it is recorded.** The transcript records what was read where it was read. Either as part
  of the entry of the message it came with - a field not covered by the digest, as
  `Given::source` is, since the model is given the message alone - or as an entry of its own,
  which gives the model nothing, like a turn's end.
- **When it is recorded.** Knowledge is recorded on the turn's context when it is read
  (`AgentWork::record_knowledge`), and with every message said afterwards; the transcript would
  record it once, at the point it was read or with the next message said.
- **A compaction and its copies.** What the compaction gives again goes into the new branch as
  `Given`; what the part kept read must be kept with it, so that a later compaction finds it.
- **Which knowledge.** Only knowledge of an entity is given again (`KnowledgeSource::Entity`); the
  rest - notes, memory, another channel's history - is recorded on the context for confidentiality
  and provenance (issues 17, 61) and need not be in the transcript for this.

## Done when

- `SummaryCompaction` finds what was read in the transcript alone; `given_read` is gone.
- AGENT-TURN.md says where the transcript records what was read.

## Decision

Decided on 2026-10-16 (see History). What the compaction gives again is meant to be the skills
alone: a skill is never summarised, but given again in full after the summary, as it is now and if
the agent may still load it - one given with `/skill` and one the model loaded alike (issues
[16](16-context-compression.md), [22](22-skill-management.md)); the `KnowledgeRestorer` of
[323](323-kinds-enumerated-in-the-core.md) only keeps the compaction from knowing skills. So the
question is not what an agent read, but which skills its transcript holds - and the two ways a
skill comes into the transcript differ without a reason:

| Way | What the transcript holds |
|---|---|
| `/skill` (`AgentChannelState::given`) | `Given { message, source }`: the skill's text as a message of its own, with what it is |
| `load_skill` (a tool) | the skill's text as the tool's result; what it is, only in the turn's `knowledge` |

**`load_skill` gives the skill as `/skill` does**, as a `Given` entry, and its result only says that
the skill was loaded and follows. Then the transcript holds every skill as a `Given` with its
source, `SummaryCompaction` reads the `Given` entries alone, and `given_read` goes. No field is
added to `TranscriptEntry::Message`, and no entry of its own records what was read.

### Given, with a source or without

A tool may give the agent something to read, with what it is or without: `Turn::give(message,
knowledge: Option<KnowledgeRef>)`, the form a compaction's replacement already has
(`AgentChannelState::compact`). With a `KnowledgeRef` the knowledge is recorded with the turn and
the entry says what it is; without one it is a message given to read and nothing more.

In the transcript both are `Given { message, source: Option<KnowledgeSource> }`: the entry says
what happened - the agent was given it, it neither said nor heard it -, whether or not it says what
it is. The source stays outside the digest, as now.

It is called `give`, not `announce`: announcing is what an agent tells people through events
(`PlanStepStarted`, `ActivityStarted`); giving is what the model is given to read, as `Turn::give`
and `AgentChannelState::given` say already. One method, extended - not a second one beside it.

### Not between the calls and their results

A tool gets the `Turn`, and `Turn::give` writes at once. But the calls of a step run first and
their results are said afterwards, as one tool message (`handle_executing_tools`): a message given
from within a call would stand between the assistant's message with the calls and their results,
which the model APIs refuse.

That nothing stands between calls and their results is a rule of the transcript, so the model
keeps it, not every tool: while a step is under way - the head of the transcript is a message with
`tool_calls` whose results are not said - `AgentChannelState::given` puts what it is given aside in
the `TurnState` (stored with it, so that a turn resumed in `ExecutingTools` loses nothing), and
`said` writes it right after the step's tool message. Outside a step - `/skill`, in a command turn
- it is written at once, as now. A tool calls `give` and cannot get the order wrong.

`create_plan` meets the same problem with a prompt part and an event and solves it in the actor,
by comparing the plan's progress before and after the calls. It stays as it is: whether it works
well is open, and it is not this issue's.

### Given, in a compaction

- **In the part summarised, with a source:** given again by the restorer of its kind, as it is
  now, unless the part kept holds it as well (`still_there`, read from the `Given` entries of the
  part kept). The summariser is told to leave its text out. One the restorer can no longer give -
  the agent may no longer load the skill - is gone; that is meant.
- **In the part summarised, without a source:** summarised with the rest. Nobody can give it
  again, and the summary is all that stays of it.
- **In the part kept:** copied as it stands, with its source or without, as every entry kept.
- **Put aside, not yet written:** a compaction runs before a call of the model, never while a
  step's calls run, so nothing is put aside then. The implementation asserts it.

### The summary

The summary is written into the new branch as `TranscriptEntry::Message`, from the agent
(`sender_id`), with the tokens of the call that wrote it, and recorded in the history as given,
without a context. Two ways to see it:

- **A `Given` without a source.** The agent did not say it in the conversation, nor hear it; it
  is given to read where the older messages stood, as the skills given again after it are. The
  replacement would then be one kind of entry, and the transcript would say of every message
  whether it was given. Its tokens are counted as now (`TokenUsage::counted`, at the compaction),
  and the measure of its call lies before the compaction's end (`TranscriptEntry::Compacted`) and
  counts for nothing on the new branch either way.
- **A message, as now.** It is the agent's own text, written by its own model; a `Given` would
  hide that it is the agent's. Nothing reads the difference today.

**Decided: a `Given` without a source.** What the transcript says of an entry is how the model is
given it, and it is given the summary to read - as the history records it already. `compact` then
writes every message of the replacement alike (`give`, the knowledge recorded where there is
one), and the summary, as a `Given`, never counts as a measure (`token_positions` reads the
agent's `Message` entries alone). The model is given the same as before: the digest and what the
resolver gives of it do not change. That it is the agent's own text stays in its sender; that it
is a summary is told, as now, by its title and its place before the compaction's end.

### Found on the way

`given_read` takes a message's knowledge from the `ContextRef` the history recorded with it, and a
turn's knowledge only grows (`Work::record_knowledge`): every message of a turn after a
`load_skill` carries the skill, not only the result that holds its text. A compaction that cuts
within a turn - in the turn under way, before a call of its model - and keeps a later message of
it counts the skill as still there, though its text was summarised, and does not give it again.
Read from the code, not shown by a test yet; the decision removes it, since a `Given` entry stands
where the text stands. The implementation shows it with a test first.

### Plan

1. A test that shows the flaw above: a skill loaded by tool in the part summarised, a later
   message of the same turn kept - the skill is given again.
2. `Given` with an optional source: `TranscriptEntry::Given { message, source:
   Option<KnowledgeSource> }`, `Transcript::give`, `AgentChannelState::given` and `Turn::give`
   take an `Option<KnowledgeRef>`; every reader of `source` follows.
3. Put aside while a step is under way: `AgentChannelState::given` keeps what it is given in the
   `TurnState` while the head is a message with calls whose results are not said, and `said`
   writes it after the step's tool message; a turn resumed keeps it.
4. `load_skill` gives the skill (`Skill::as_given`, `Skill::knowledge`) instead of returning its
   text, and answers that it was loaded; its result is classified as the turn's label, the
   skill's classification goes with the message given.
5. The summary is a `Given` without a source: `AgentChannelState::compact` gives every message
   of the replacement as `Given`, the source where there is one.
6. `SummaryCompaction` finds what was given in the `Given` entries alone, as *Given, in a
   compaction* says; `given_read` and the map by message go, and with them the last place that
   builds the model's input from the history (AGENT-TURN.md, *What is held in memory*).
7. Documents: SKILLS.md (how a skill is loaded and given again), AGENT-TURN.md (*Compaction*, the
   transcript's entries, the reader of the history named for 389), TOOLS.md (what a tool may give
   the agent, and when it is written).

To check while implementing:

- How the web UI shows a `load_skill` result (`ToolPresentation` `skill`): if it shows the text
  from the result, the chat no longer has it, since what is given is not published.
- What a turn given up as `Lowered` makes of a message given in it (`Message::names_only`) - the
  same holds for `/skill` within a turn today.
- That the results of a step are always one tool message, so that what is given follows all of
  them.

Stored data: agent states and transcripts are to be deleted - `TurnState` gains a field, and
`Given` takes its source as optional.

## History

- **2026-10-16** — Opened from the discussion of
  [387](387-agent-history-window-without-compaction.md): the history is informational and never
  used to build the model's input; this is one of the two places that do it today.
- **2026-10-16** — Discussed: what is given again is the skills, as 16 and 22 decided. `load_skill`
  gives its skill into the transcript as `/skill` does, after the tool message of its step, and
  the compaction reads the `Given` entries alone; see *Decision*. A flaw of `given_read` was found
  on the way: a turn's knowledge only grows, so a skill counts as kept where only a later message
  of its turn is.
- **2026-10-16** — Refined: a tool gives through `Turn::give`, with a `KnowledgeRef` or without,
  and the model puts it aside while a step's calls run; `Given` takes an optional source.
  `create_plan` stays as it is. How a `Given` is treated in a compaction is part of this issue,
  and so is the decision whether the summary is a `Given` without a source.
- **2026-10-16** — Decided: the summary of a compaction is a `Given` without a source.
- **2026-10-16** — Done as planned. A test showed the flaw first: a skill loaded by tool in the
  part summarised was not given again once a later message of its turn was kept. `Given` takes an
  optional source; `Transcript::give`, `AgentChannelState::given` and `Turn::give` take an
  optional `KnowledgeRef`. While the calls of a step run, what is given waits in
  `TurnState::given_after_step` and is written after the step's tool message, also after one
  answering abandoned calls. `load_skill` gives the skill's text and answers with its name and id,
  classified as the skill, as `list_skills` names it. `compact` gives every message of the
  replacement as `Given`, the summary without a source. `SummaryCompaction` finds what was given in
  the `Given` entries alone; `given_read` is gone, and with it the last place that built the
  model's input from the history. Checked: the web UI never showed a loaded skill's text and reads
  `name`, `skill_id` and `status` from the result; a turn given up as `Lowered` gives a message
  given in it no more, as it gave the text of the result no more; a step's results are one tool
  message. Stored agent states and transcripts are to be deleted.
