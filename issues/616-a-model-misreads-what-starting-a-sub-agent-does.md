# 616 — A model misreads what starting a sub-agent does

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Tools and approvals
**Summary:** A model that starts a sub-agent does not take from the tools' texts that the brief is the whole task and already on its way: in an experiment Gemma started a sub-agent and at once asked an additional question, as if the brief had not yet set the sub-agent to work.

## The problem

Observed by the maintainer in an experiment with Gemma: the agent called `start_subagent` and at
once asked an additional question. The effect of the brief it had passed - that the sub-agent is
given it and works on it at once - was evidently not clear to the model.

Not yet known: whether the question went to the sub-agent - `ask_subagent` in the same turn - or to
the user. The two point to different gaps; the transcript of the experiment would tell.

What the model is told (`caelumora-subagents/src/prompts.yaml`):

- `start_subagent`: "Start a sub-agent on a task: a new agent … working on the brief apart, in a
  thread of its own at this call. It returns at once with the thread; what the sub-agent answers is
  given to you to read once it is there. Write the brief so that it can be worked on without what
  was said here. Ask it again with `ask_subagent` in the turn in which you read its answer - left
  without a word then, it is ended -, or end it with `end_subagent` once its work is done."
- its result: `{"thread": "<id>"}` (`StartSubagentTool::execute`).
- `ask_subagent`: "Give a sub-agent you started more to read - a question, a correction, the next
  task -, in the turn in which you read its answer …"

What happens with a question asked right after the start: the channel queues it behind the brief
(CHANNELS.md *Delegating into a thread*: "a further brief waits behind it"); the sub-agent answers
the brief first, then the question, each answer given back on its own. Nothing breaks, but the
question is answered late, out of context, and costs a second answer the agent then has to give a
word on.

## What the texts leave open

- **That the brief already is the request.** "Working on the brief" does not say plainly that the
  sub-agent has it and starts now; a model that knows chat APIs may take `start_subagent` as
  opening a conversation in which it then has to ask.
- **What to do next.** Nothing says: nothing more is needed now; go on with other work or end the
  turn, and the answer comes in a later turn. "In the turn in which you read its answer" is the
  only hint, and it is about asking again, not about waiting.
- **The result says nothing.** `{"thread": …}` gives an id and no state. A model reads a tool's
  result more closely than its description, right where it decides what to do next.
- **"Ask it again"** in `start_subagent` names the next tool before the model has any answer -
  likely to invite exactly the call observed.

## Considerations

1. **The descriptions.** `start_subagent` says first that the brief is given to the sub-agent at
   once and is all it gets, that it works on it alone - nobody but the agent gives it anything in
   its thread (`Rejection::ClosedCircle`) -, and that its answer is given in a later turn; the
   hint at `ask_subagent` moves to the end and says "after you read its answer". `ask_subagent`
   says it is not for adding to a brief whose answer has not come.
2. **The result.** The tool returns a text beside the thread, from `prompts.yaml`, e.g. "Started:
   it has the brief and works on it. Its answer will be given to you to read; nothing more is
   needed now." Small models follow results better than descriptions.
3. **Refusing an early ask.** `ask_subagent` refused while the brief before is still open, with a
   result that says why. A question that adds to a brief has a use - a correction sent quickly -
   but is rare, and a correction can be the next brief. It changes behaviour of the writer, not only
   texts, and needs the maintainer's word.
4. **Texts of the model's own.** `examples/prompts/gemma4/agents.yaml` holds texts for Gemma of the
   agents' namespace; the sub-agents' texts could get a variant there too (LLM-MODELS.md). Only if
   1 and 2 do not suffice: a text that only one model understands is a weaker text.

Recommended: 1 and 2 together, tried again with Gemma and one other model; 3 only if the call
still comes.

## Done when

- Which question it was is known from the transcript, or the texts cover both readings.
- The descriptions and the result of `start_subagent` say that the brief is the task and on its
  way, and what to do meanwhile; covered by the tests of the tools' texts.
- Tried with Gemma: a sub-agent started is not asked again before its answer came - recorded here.

## History

- **2026-11-27** — Opened on the maintainer's observation from an experiment with Gemma.
