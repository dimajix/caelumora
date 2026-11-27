# 619 — A sub-agent gives back everything it said

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** What a sub-agent gives back to the agent that started it is every word it said in the turn answering its brief - each remark before a tool call as well as its answer, joined, without a bound -, and nothing tells a sub-agent that its answer goes back or how it should be written, so the main agent reads much more than a deliberate, summarising result.

## The problem

The delegate of a thread (issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md)) takes
the recipient's first turn that ends after the brief was given as its answer, and gives back what
others hear of that turn: every message of it that is heard (`Message::is_heard` - a user or
assistant message, not `internal`, with words or attachments), their texts joined
(`DelegateActor::answer`, `caelumora-kernel/src/actor/delegate.rs`).

- **In it:** every remark a model writes before a tool call ("First I look at X", "Now the
  tests"), as well as the turn's answer. A turn of many steps gives back many of them.
- **Not in it:** the results of tools - no file read, no output of a command -, and the steps of a
  plan, which are `internal`.
- **Bounded** to one turn, but not in length.

The sub-agent learns nothing of this from the system. It is given the brief as anything given to
read is: "@assistant gave you this to read: …" (`given_by`, `caelumora-domain/src/prompts.yaml`).
That its answer goes back, and how it should be written, only its personality can say:

- `caelumora-tui/defaults/helper.yaml`, the personality of the terminal's sub-agents (issue 609),
  says "answer with what came of it … completely and briefly, since your answer is all of your work
  the assistant reads" - which the delegate does not hold to: the assistant reads every remark of
  the turn too.
- Other personalities started as sub-agents are not told that they are one.

## Why it matters

- The point of a sub-agent (issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md)) is that the
  main agent's context holds the result and not every step of the work. Remarks before tool calls
  are steps.
- What a sub-agent gives back should be written for its reader: a deliberate, if need be
  summarising, answer - which the sub-agent, who knows its work, writes best.
- A long answer fills the main agent's context as fast as doing the work itself would.

## Considerations

- **A. Only the turn's answer is given back**: its last message, the one that calls no tool - what
  AGENT-TURN.md (*What leaves the agent*) already calls the turn's answer. The delegate has to know
  it: by a mark on what the turn says, or as the last heard message of the turn. Claude Code's task
  tool gives back the same, and it is what the helper's personality already promises.
- **B. The sub-agent is told what goes back.** The brief is given in a framing of its own instead
  of the general `given_by`, along the lines of: "Your answer to this brief - your last message,
  calling no tool - is given back to @assistant and is all it reads of your work: make it complete
  and brief." Then every personality started as a sub-agent knows it, not only the terminal's
  helper, whose prompt can then leave it out. An external sub-agent (issue
  [591](591-an-external-agent-cannot-be-a-sub-agent.md)) answers a request with one answer anyway,
  but is given the brief as text too, so B helps it as well.
- **C. A bound on the length, as a safeguard.** What goes beyond it is cut, with a notice saying
  so; whoever has to deliver much writes a file into the workspace or publishes an artifact and
  names it in its answer.
- **D. A model summarises what came back.** A call more, and what it leaves out is lost; the
  sub-agent knows its work best and summarises it better itself.

Recommended: A and B together, C as a safeguard, not D.

Open: where the bound of C is set - a constant of the kernel, or a setting of the plugin's section
(`plugins.subagents`), as its other limits are (issue
[597](597-sub-agents-are-bounded-by-a-list-of-personalities-alone.md)) - and what the delegate gives
back where an answer was cut and the turn had no answer but remarks.

## Done when

- The delegate gives back the answer of the turn answering a brief alone, bounded as decided.
- The recipient of a brief is told that its answer goes back and how to write it, whatever its
  personality; the helper of the terminal is adjusted to it.
- Covered by tests of the delegate; CHANNELS.md (*Delegating into a thread*) and TOOLS.md
  (*Sub-agents*) say what is given back.

## History

- **2026-11-27** — Opened from the discussion of issue 591: what a sub-agent gives back, read in
  the code, is everything it said in the turn, not a deliberate answer.
