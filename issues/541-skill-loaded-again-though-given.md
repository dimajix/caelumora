# 541 — A skill given to the agent is loaded again and stands twice in its transcript

**Status:** done
**Type:** feature
**Priority:** low
**Area:** Context and knowledge
**Summary:** After a participant gave an agent a skill with `/skill`, models call `load_skill` for it anyway, and the skill is given a second time; `load_skill` could answer for a skill that stands in the transcript already without giving it again.

## The problem

The experiment of issue [539](539-given-messages-framed-by-their-giver.md)
(`work/given_framing/RESULTS.md`) gives the agent a skill with `/skill` and lets another
participant ask for what it is about. `gemma4:cloud` calls `load_skill` for the skill in every run,
`gemma4:26b` in about half of them - however what is given is framed: under its title in brackets,
after who gave it, in an element of the kernel, as a system message where it stands. Offered the
tools of the `skills` component, no framing tells the models that the skill is there already.

`load_skill` then does what it always does ([SKILLS.md](../docs/SKILLS.md)): it gives the skill's
text, after the results of the step, as a `Given` entry. The skill stands twice in the transcript.

## Why it matters

- **A call more** in each of these turns, and a step more before the agent answers.
- **The context twice the skill's size**, until a compaction - and a compaction gives each skill
  again only once anyway (issue [413](413-compaction-restores-unused-skills.md)).
- **No wrong answer**: the models follow the skill once they loaded it again. The cost is all there
  is.

## Considerations

- **`load_skill` answers "already given above".** If the transcript of the agent holds a `Given`
  entry for the skill, as it is now, the result says so and gives nothing. It needs to know what the
  transcript holds - whether a tool of a component may see that, or the turn decides, is the first
  question; the compaction already asks it, from the other side ("unless the part kept still holds
  it").
- **A skill changed since it was given** is given again: what is compared is the skill as it is
  now, not its name.
- **The tool's description asks not to load a skill given above.** Cheaper, but no text the
  experiment tried made the models see it as given; whether a sentence in the tool does is to be
  measured, with both models.
- **Leave it.** Loading again costs a call and the space of a skill, nothing else.

## The plan

Decided with the maintainer, in steps of their own:

1. **Giving gives only what does not stand given.** As `give_parts` gives a prompt part only where
   it does not stand (issue 537), `AgentChannelState::given` - and `Turn::give` with it - gives
   knowledge only if the same knowledge does not stand given already with the same text and
   classification, on the active path as the model is given it or put aside for after the step,
   and answers whether it gave it; the knowledge is recorded with the turn either way. What stands
   given is read by one function, `given_as_they_stand`, of which `parts_as_given` keeps the parts.
   `load_skill` answers `given above` then, `/skill` that the agent has the skill already.
2. **A measurement** under `work/given_framing/`, with `gemma4:26b` and `gemma4:cloud` alone:
   whether a model answered `given above` follows the skill then, or loads it again.

## Done when

- A skill that stands in the agent's transcript, as it is now, is not given to it a second time by
  `load_skill`; or it is decided that it may be.

## History

- **2026-11-06** — Opened from finding 5 of the experiment of issue 539, step 2.
- **2026-11-06** — Planned with the maintainer (*The plan*): the giving decides, not the tool.
- **2026-11-06** — Step 1 done: `AgentChannelState::given` - and `Turn::give` - gives knowledge
  only where the same does not stand given with the same text and classification, on the active
  path as the model is given it (`given_as_they_stand`, of which `parts_as_given` keeps the parts)
  or put aside for after the step, records it with the turn either way and answers whether it gave
  it. `load_skill` answers `given above` then, `/skill` that the agent has the skill already. What
  is alike is one function for parts and knowledge (`alike`), and which knowledge an entry gives
  one method (`TranscriptEntry::knowledge`), which the compaction's cut reads too. The description
  of `load_skill` tells the model that the text follows the result unless it stands above already.
- **2026-11-06** — The findings of step 1, decided with the maintainer to be addressed before the
  measurement: the module docs of the skills' tool and command say what they do since issues 389
  and 417. The compaction's cut reads what was given as the model is given it
  (`given_as_they_stand`, now public), so a gift of a turn given up as `Lowered` no longer stands
  for one in the part replaced; it keeps telling an entity by its kind and id, unlike the giving,
  which compares the text: a compaction gives again what it takes away, and a skill changed since
  is loaded anew - and given then.
  A function of its own for the turns given up as `Lowered`, which the cut, `given_as_they_stand`
  and the resolver each collect in one line, was weighed and not taken: a self-explaining line is
  not worth a name.
- **2026-11-06** — Step 2 done, and with it the issue: the measurement
  (`work/given_framing/RESULTS.md`, *Answered `given above`*). Answered `given above`, `gemma4:26b`
  and `gemma4:cloud` follow the skill at once in 20 of 20 runs each and never load it again. The
  new description of `load_skill` does not keep them from loading a skill alice gave (`gemma4:26b`
  12 of 20, `gemma4:cloud` 20 of 20, as before): a model still calls it once, but the skill no
  longer stands twice.
