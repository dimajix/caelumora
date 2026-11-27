# 619 — A sub-agent gives back everything it said

**Status:** in progress
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

## Decision

Decided by the maintainer on 2026-11-27, after weighing the options below.

**D1 - The answer is the turn's last message, if it is one.** The delegate keeps the last message
of each turn of the recipient it sees, and gives back its text if it is the turn's answer
(`Message::is_answer`: an assistant message, not internal, calling no tool) and has words; else
that the turn ended without an answer. It keeps collecting what is said, since a turn's end read
from the history after a restart carries no messages. A remark followed by calls hidden from
others alone (private notes, agent memory) and an end without words looks like an answer: rare,
documented and held by a test, rather than a mark of its own on the message.

**D2 - Remarks are no answer.** A turn that ends with remarks but no answer - one cancelled while
it called a tool included - gives back that it ended without an answer. An answer cut short by a
cancel is given back as far as it came only of an external agent, which says what it has; an
internal agent's is only shown while it streams, never said, so its turn ends without an answer
(corrected on 2026-11-27, see *History*).

**D3 - A bound of 16 000 characters, a constant of the delegate.** What goes beyond it is cut at a
character boundary, with a notice of the kernel's texts (`delegation_cut`). No setting: nobody
would set it.

**D4 - The brief is framed of its own.** The recipient takes a brief in as given of its own kind
(`GivenSource::Brief`), and its profile frames it with the kernel's `given_brief`: who gave it, and
that its answer alone - its last message, calling no tool - goes back and is all the giver reads
of its work. The framing is the recipient's, since the channel admits a brief only as given; it
reaches an external agent as well. The terminal's helper no longer says it itself.

**D5 - An answer written with a call that ends the turn is not taken for one.** A tool of
`turn_end` (`stop`, `bye`) ends the turn with its tool step, so an answer written in the message
that calls it is no answer and is not given back. Decided on 2026-11-27, after the review of step
2 found it: the terminal's helper is shipped without `turn_end` - a sub-agent's turn ends with an
answer calling no tool anyway -, and the brief's framing (D4) tells every sub-agent to answer
without calling a tool. A personality with `turn_end` started as a sub-agent can still lose its
answer so; documented.

Rejected:

- **The turn marks its answer on the message** - a stored field for a rare case.
- **A setting of `plugins.subagents` for the bound** - passed through the channel for a value
  nobody chooses.
- **A model summarising what came back** (D of *Considerations*).
- **The model's last message as the answer, whether it calls tools or not** (for D5) - a turn
  cancelled while calling a tool would give back its remark again, against D2, and an answer
  without words, which is not published, would give back the remark before it.
- **Calls that end the turn as a concept of their own**, marked on the tool and the call so that
  an answer may make them (for D5) - a new concept of the domain for what the framing tells.
- **Brief and what comes back told apart in the event** - an event or a field of its own: it
  changes stored history and every place handling `GivenToRead`, for what the envelope tells
  already.

## Plan

The interfaces as agreed on 2026-11-27. Each step a commit of its own, reviewed before the next.

### Step 1 - The answer and the brief named, without a change of behaviour

The rule of a turn's answer and the rule of a brief each move to one place.

```rust
// caelumora-domain/src/message.rs
/// Whether this is a turn's answer: the agent's own words, not a step of a plan, calling no tool.
pub fn is_answer(&self) -> bool

// caelumora-domain/src/actor/envelope.rs
/// Whether it gives a brief (issues 557, 619): a `GivenToRead` its thread's own delegate sends,
/// as against what that delegate gives back where the thread was opened.
pub fn is_brief(&self) -> bool
```

`Turn::say` uses `is_answer`; the channel's routing and `admit_given` use `is_brief`.

### Step 2 - The answer alone, bounded (D1-D3)

`DelegateActor` keeps the last message per turn instead of every heard one, gives back its text if
it is an answer, cut at `MAX_ANSWER` (16 000 characters) with `kernel.delegation_cut`. Tests of the
delegate; CHANNELS.md (*Delegating into a thread*) and TOOLS.md (*Sub-agents*).

### Step 3 - The brief framed (D4)

```rust
// caelumora-domain/src/transcript.rs
pub enum GivenSource { /* ... */
    /// A brief delegated to the agent in its thread (issues 557, 619).
    Brief }

// caelumora-domain/src/agent/channel_state.rs
/// Records `message`, a brief given to the agent, as given (`GivenSource::Brief`).
pub fn given_brief(&mut self, thread: ThreadId, message: Message)
```

The intake takes a brief in by `Envelope::is_brief`; `ModelProfile::given_to_read` is given the
source and frames a brief with `kernel.given_brief`; the helper's prompt is adjusted and the helper
shipped without `turn_end` (D5). Tests of the framing, also for an external agent; AGENT-TURN.md
(*What is given to read*).

### Step 4 - The findings of step 2, without a change of behaviour

On the maintainer's decision of 2026-11-27, the two findings of the review of step 2 are part of
this work:

- **One way to cut a text after a number of characters.** It is written out at four places
  (`char_indices().nth(n)`, then a cut, marked with `…`: the delegate's `bounded`,
  `caelumora-tui/src/printed.rs` `beginning`, `caelumora-tui/src/screen/words.rs` `shortened`,
  `caelumora-tool-workspace/.../grep_files.rs` `shown`), and as `chars().take(n)` at four more
  (`caelumora-agents/src/actor/external_agent.rs` `notice`, marked; the web search's
  `tavily.rs` twice and `caelumora-model-ollama/src/client.rs`, unmarked). One function of the
  domain takes their place; its interface is put before the maintainer as the step begins.
- **The delegate's tests build their messages from `make_message`**, the `brief` helper too.

## History

- **2026-11-27** — Opened from the discussion of issue 591: what a sub-agent gives back, read in
  the code, is everything it said in the turn, not a deliberate answer.
- **2026-11-27** — Decided by the maintainer (*Decision*), planned in three steps (*Plan*); in
  progress.
- **2026-11-27** — Step 1 done: the rule of a turn's answer (`Message::is_answer`) and of a brief
  (`Envelope::is_brief`) each in one place, without a change of behaviour.
- **2026-11-27** — Step 2 done: the delegate gives back the turn's answer alone, cut after 16 000
  characters with a notice (`MAX_ANSWER`, `kernel.delegation_cut`); a turn whose last message is
  no answer ended without one.
- **2026-11-27** — D2 corrected, as the review of step 2 found in the code: an internal agent's
  answer cut short by a cancel is not said, so it is not given back; only an external agent's is.
- **2026-11-27** — D5 decided by the maintainer (an answer written with a call that ends the turn);
  step 4 planned for the two findings of the review of step 2.
- **2026-11-27** — Step 3 done: a brief is taken in as one (`GivenSource::Brief`,
  `AgentChannelState::given_brief`) and framed with `kernel.given_brief`, also for an external
  agent's remote agent; the terminal's helper says it no longer itself and has no `turn_end` (D5).
