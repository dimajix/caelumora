# 541 — A skill given to the agent is loaded again and stands twice in its transcript

**Status:** open
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

## Done when

- A skill that stands in the agent's transcript, as it is now, is not given to it a second time by
  `load_skill`; or it is decided that it may be.

## History

- **2026-11-06** — Opened from finding 5 of the experiment of issue 539, step 2.
