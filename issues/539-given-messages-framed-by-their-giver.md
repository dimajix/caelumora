# 539 — What is given into an agent's transcript is framed by whoever gives it

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** What is given into an agent's transcript - a skill, a compaction's summary, a prompt part - takes the role and sender its giver chooses, so the model cannot tell given context from what was said, and a skill a user loaded is given again after a compaction as the agent's own.

## The problem

An entry `TranscriptEntry::Given` is a `Message` its giver builds, with a role and a sender of its
choosing:

- a skill the model loads (`load_skill`, `caelumora-skills/src/component/tool.rs`): role `User`,
  sent by the agent itself, so unmarked;
- a skill a user loads (`/skill`, `caelumora-skills/src/component/command.rs`): role `User`, sent
  by that user, so marked as their message - deliberately, as their act;
- a compaction's summary (`caelumora-compaction/src/compactor.rs`): role `User`, sent by the
  agent, unmarked, recognisable only by its title in brackets;
- a skill given again after a compaction (same file): sent by the agent, whoever loaded it - a
  skill a user loaded loses that it was theirs;
- a prompt part at `Given` (issue [537](537-prompt-parts-placed-anew-every-turn.md)): a system
  message of its component, where the model keeps one (`SystemMessages`), else a user message.

The channel's notices of who joined or left, by contrast, are worded and framed by the
`ContextResolver` alone, and the profile's `framing_explanation` tells the model how others'
messages are marked.

## Why it matters

- **A model cannot tell context from conversation.** A summary or a skill given as an unmarked
  user message reads like something the agent said to itself; Claude Code marks everything its
  harness inserts the same way (`<system-reminder>`) and explains that once in its system prompt.
- **Who gave a skill is lost** at the first compaction, though the transcript recorded it.
- **Every giver repeats the choice**, and a new one - the files of issue
  [538](538-workspace-knowledge-without-a-place.md) - would choose again.

## Considerations

- **The resolver frames what is given**, as it frames the notices: a system message where the
  profile keeps one, else a user message marked as the kernel's, the marking explained once in
  `framing_explanation`. A giver says what it gives and as whose act, not how it reads.
- **A user's act stays theirs.** A skill a user loaded is framed as given at their request; the
  compaction gives it again with the same origin. The origin belongs to the entry (`GivenSource`
  or the message's sender), not to the restorer.
- **The parts at `Given`** already take the role the profile allows; they would take the common
  frame too, so that one rule serves all.
- **Experiments.** Whether small models follow a marked user message as well as an unmarked one
  is to be tried, as `work/quiet_agents` tried the mentions.

## The plan

Decided with the maintainer, in steps of their own:

1. **The resolver frames what is given.** `ModelProfile::given_to_read` gives an entry
   `TranscriptEntry::Given` as a system message - where the model keeps one, as the channel's
   notices are -, never marked as said, and words who gave it if another participant did
   (`kernel.given_by`: "@alice gave you this to read:"). `given_path` and `given_messages` give
   what is given by it and what is said by `ModelProfile::given`. The givers - the skills' tool and
   command, the compaction - write their messages in the role `System`, as the parts are. The
   explanation of the markers (`kernel.framing`) says that what is given begins with its title in
   brackets and is no participant's message; it is given where senders are marked, as now.
2. **An experiment** under `work/` tells whether a model follows what is given under its title as
   well as in an element of the kernel, with `gemma4:26b` or `gemma4:cloud` alone.
3. **The format configurable**, if the experiment finds a model it helps: `GivenFormat` of the
   `ModelProfile` - `titled`, the default, or `tagged`, `<given from="@alice">...</given>` -, a
   parameter `given_format` of `language_model` and `a2a_agent` beside `sender_format`, not coupled
   to it; the explanation branches on it, and `tagged` escapes `</given` as `xml` does
   `</message`.
4. **Who gave it outlasts a compaction.** `Cut::wanted` names with what was given the message it
   was first given in, whose sender the compaction gives it again as.

## Done when

- Everything given into a transcript is framed by the `ContextResolver` by one rule, explained to
  the model once.
- A skill a user loaded is given again after a compaction as theirs.

## History

- **2026-11-06** — Opened from the comparison of Claude Code's session logs with Caelumora's
  concepts that led to issue 537 (point C there).
- **2026-11-06** — Planned with the maintainer (*The plan*): the title in brackets as the mark, who
  gave it from the message's sender, a format of the kernel's element only as a setting of its
  own, after an experiment with `gemma4:26b` or `gemma4:cloud`.
- **2026-11-06** — Step 1 done: `ModelProfile::given_to_read` frames every `Given` entry, for the
  agent's model and for a remote agent alike - its reasoning and notes as for a message said, but
  never marked -; another participant's gift is worded by `kernel.given_by`, and `kernel.framing`
  names the title in brackets as the mark. The givers choose no role: what they give is a system
  message, as `Message::new` makes one, and the resolver gives it so. As a summary that begins a
  branch would then have stood among the system messages at the start, which a model merges into
  its system prompt, the maintainer decided that the system prompt is the parts at `Start` alone
  (`place_system_messages` is given what follows them): a message of the transcript is none of it,
  a channel's notice at the start neither, which had been before. A role of its own for what is
  given (`MessageRole::Given`) was weighed and not taken: the role is stored, served and mapped by
  every backend, and `System` already means what is no participant's words.
  A `system_prompt` given into the transcript names its `title`, the mark of what is given.
  Left open: an external agent is handed a gift as `Said` with the giver as its `sender`, which A2A
  ignores; a protocol that carries the sender would show it as the giver's message, to be decided
  when there is one.
- **2026-11-06** — Decided with the maintainer and done, as a step of its own: `ModelProfile::given`
  is `as_said`, beside `given_to_read`, and the resolver's dispatch between them `framed`. *Given*
  keeps the meaning issue [398](398-given-means-four-things.md) found for it - given to the agent
  or its model -; the two methods name which messages they frame.
- **2026-11-06** — Decided with the maintainer and done, as a step of its own: the role of a prompt
  part is its frame's (`Placement::Frame { at, role, component }`), no longer a field of
  `PromptPart` beside a placement that gives it no meaning: a part given into the transcript is
  framed as what is given to read, a system message, whatever role it named.
- **2026-11-06** — Step 2 done: the experiment (`work/given_framing/RESULTS.md`). No framing
  makes `gemma4:26b` or `gemma4:cloud` take a skill a user gave for given: `gemma4:cloud` loads it
  again in 20 of 20 runs under every variant, `gemma4:26b` in 9 to 15 of 20, within the noise.
  Following a skill the model loaded, going on from a summary and telling the senders apart hold in
  every variant; a block a participant forges is obeyed alike under all of them. A first run that
  offered `load_skill` alone had found `<given>` taken for given by `gemma4:cloud`; offered the
  tools of the `skills` component, as Caelumora offers them, it is not. So step 3 is not taken: the
  format stays the title in brackets, and nothing is configured that a real configuration does not
  show a use for. Found on the way: loading again a skill that stands in the transcript gives it a
  second time; whether `load_skill` should answer that without giving it is the skills' question.
