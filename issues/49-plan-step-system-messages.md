# 49 — Plan step instructions are system messages in the middle of a turn

**Status:** done
**Area:** Tools & approvals

## The problem

While an agent executes a plan, `AgentActor::plan_step_message` and
`plan_finalize_message` (`caelumora-runtime/src/actor_impl/agent.rs`) push
the instruction for the next step as a message with `MessageRole::System`
into the middle of the turn.

Some models' chat templates merge every system message into the system
prompt and move it to the front. For them the instruction is not where it was
put, right before the step it is about, but part of the system prompt:

- its position, and so its meaning ("now do step 3"), is lost;
- the system prompt changes with every step, which invalidates the whole
  prompt cache ([47](47-prompt-parts-with-provenance.md)).

## Why it matters

Plan execution is the one place where the runtime deliberately steers the
model mid-turn. That it depends on the chat template whether the steering
lands where it was meant is fragile, and hard to see from outside.

## Considerations

- [47](47-prompt-parts-with-provenance.md) settled the same question for
  prompt parts: a part in the middle of the context is given as a user
  message, not a system message. The plan step instructions could follow the
  same rule, clearly framed as instructions of the runtime.
- The instructions are internal messages (`internal: true`) in the turn, not
  prompt parts: they are recorded in the context. Whether they should become
  prompt parts, or stay messages with another role, is part of the decision.
- Alternatively the role could be left to the LLM backend, which knows its
  model: a system message where the model keeps it in place, a user message
  otherwise. Today there is only the Ollama backend, which passes the role on
  unchanged.

## Done when

- Plan step and finalize instructions reach every model where they were
  placed, independently of how its chat template treats system messages.

## History

- **2026-09-25** — Issue created while implementing
  [47](47-prompt-parts-with-provenance.md), prompted by the observation
  that some models merge all system messages and handle them first.
- **2026-09-30** — Decided with the maintainer, before implementing: the
  question is part of a larger one - how the model's input is assembled and
  what is stored of it.
  - **Findings.** The instructions are worse than described above: being
    turn messages, `record_turn` appends them to the transcript, so every
    *later* turn gets them too, as system messages with `sender_id:
    ActorId::None` in the middle of the context. How something is shown to
    the model is decided in four places - the role of a prompt part by the
    `ContextProvider`, reasoning by `Turn::full_context` and again by
    `AgentChannelState::resolve`, the role of a plan instruction as a
    constant in `agent_turn.rs`, tool results by the Ollama client - and
    `full_context` and `resolve` assemble the same sequence twice.
    `TurnState::context` holds copies of the whole transcript and is stored
    after every step of a turn. (What another participant says reaches an
    agent as a `user` message already - the moderator's
    `as_user_messages`; what is missing is the sender's name, which no
    backend passes on. An issue of its own, once the resolver below
    exists.)
  - **Options for what is stored.** (A) Facts, and every rule of
    presentation as one more field of `ContextRef` (the line of
    [63](63-reasoning-in-model-input.md)) - every rule a field, and both
    paths re-implement it. (B) The input as given - it is no growing prefix
    (the part before the turn's input moves, `current_turn` changes earlier
    entries), so it would have to be stored per turn, quadratic in the
    transcript's length (see [16](16-context-compression.md)'s history).
    (C) Facts, a frame and one resolver - chosen:
  - **The frame.** The turn holds a frame instead of copied items: the
    transcript digest it began with, the parts placed in it, and the
    model's rules - what is recorded as `ContextRef` with every message.
    Whether frame and `ContextRef` become one type is to be settled when
    implementing; that requires the frame to know which entries are the
    turn's own from the start instead of counting `turn_entries` at commit.
    The model's rules are a closed set, as data (the decision of
    [16](16-context-compression.md)'s history, generalised): a profile the
    `LLMClient` declares, of which `ReasoningHistory` becomes one field; the
    only new rule for now is how system text in the middle of the
    conversation is given. It is fixed in the frame when the turn begins
    (`next_turn`), like the classification, and no longer handed to
    `Turn::new`.
  - **`ContextResolver`** (`caelumora-model`, a module of its own): resolves
    a frame against the stored data - the transcript and the prompt texts -
    into what the model is given. One implementation for both uses:
    `resolve(&ContextRef, agent)` for a recorded message (the context
    panel), `current(&TurnState, agent)` for the next model call of the
    turn under way. `Turn::full_context` delegates to it. Not a method of
    `ContextRef`, which holds only references, and not of
    `AgentChannelState`, which is to stay a plain state: it keeps
    `recorded_context_of`; `resolve` and `seen_context_of` move out. The
    `ContextProvider` builds the frame, the resolver resolves it - hence not
    `ContextBuilder`.
  - **`SeenContext` becomes `ResolvedContext`.** Still needed - REST reads
    both the items and the reference (`knowledge`, `transcript`) - and with
    the resolver it is also the input of the turn under way, which nobody
    has seen yet.
  - **The plan instructions become prompt parts of the turn**, placed after
    the turn's entry they follow, not transcript entries: they are text of
    the runtime for the model, like the other prompt parts, and follow the
    rule of [47](47-prompt-parts-with-provenance.md) - a framed user message
    in the middle of the context, or a system message where the model's
    profile says it keeps one in place. Later turns no longer get them; the
    plan stays visible through the `create_plan` call in the transcript. A
    `PromptRef` then applies from a message of the turn on, not to the whole
    turn, like `TurnKnowledge::before`. Rejected: a transcript entry of its
    own kind - no fake message any more, but later turns would still get
    every instruction, and the transcript would mix record and scaffolding.
  - **Order:** first the frame and the resolver, as a refactoring without
    new behaviour; then this issue on top, with the first profile rule.
    Costs: the stored form of `ContextRef` and `TurnState` changes (no
    serde defaults, [108](108-serde-defaults-for-legacy.md)), the
    `ContextProvider` returns a frame instead of items.
- **2026-09-30** — First step done, the refactoring without new
  behaviour. `caelumora_model::context` holds what the model is given and
  what is recorded of it: `PromptRef`, `ContextItem`, `KnowledgeRef` and
  `KnowledgeSource`, `ReasoningHistory` (moved from `message`), and new
  `ModelProfile`, `ContextFrame`, `PromptPart`, `ContextRef`,
  `ResolvedContext` (formerly `agent::SeenContext`) and `ContextResolver`.
  - **Frame and `ContextRef` stay two types.** The frame names the
    transcript the model is given; a `ContextRef` names the transcript
    before its message, of which the turn's own entries are the last
    `turn_entries`. `ContextFrame::reference` makes the one from the other
    when the turn is taken over.
  - **The `ContextProvider` places, the turn keeps references.**
    `Turn::set_context(transcript, Vec<PromptPart>)` - the transcript's
    digest and each part with the entry it follows - replaces
    `set_context(Vec<ContextItem>)`: a frame cannot hold a conversation
    that is not a prefix of the transcript, so the provider is not given a
    way to hand one in. The transcript is the provider's to name (what
    compression will change), the profile the turn's.
  - **The profile is still handed to `Turn::new`**, unlike planned above:
    the maintainer's choice - it is a property of the agent's model, like
    the rest of what `Turn::new` is given. `next_turn` copies it into the
    frame, and only the frame's is applied, so that a turn resumed after
    the model's configuration changed is given its context - and recorded -
    by the profile it began with.
  - **`ContextItem::Conversation` lost its digest**, which only placing
    the parts read; the turn's own messages have none yet while it runs.
    `ContextItem::prompt` is gone with it.
  - `LLMClient::reasoning_history` became `LLMClient::profile`; the
    `ollama_client` parameter `reasoning_history` is unchanged.
    `Turn::context` is gone,
    `Turn::full_context` fails if the frame cannot be resolved, and so does
    the model call. `AgentChannelState` keeps only `recorded_context_of`.
  - The stored form of `TurnState` (`frame` for `context`) and of
    `ContextRef` (`profile` for `reasoning`) changed, without serde
    defaults ([108](108-serde-defaults-for-legacy.md)).
- **2026-09-30** — Resolved on top of the refactoring.
  - **Instructions of the runtime are prompt parts of the turn.**
    `Turn::place_turn_prompt` places a part after the messages the turn has
    produced so far, as a `TurnPromptRef` in the frame - by that number,
    since those messages are not in the transcript yet. `advance_plan` uses
    it for the step and finalize instructions (`plan_step_instruction`,
    `plan_finalize_instruction`, as text now) instead of pushing internal
    system messages. `ContextFrame::reference` turns every instruction
    placed before a message into a `PromptRef` after the transcript entry
    it followed, so that the context panel shows it where it was given.
    Later turns are not given them; the transcript holds none.
  - **The first rule of the profile beyond reasoning:**
    `ModelProfile::system_messages` (`SystemMessages`: `only_at_start`, the
    default, or `anywhere`), the `ollama_client` parameter
    `system_messages`. The resolver applies it to the resolved items: a
    system message after the first item that is none - prompt part or
    transcript message - is given as a user message unless the model keeps
    them anywhere. It also covers the plan instructions that transcripts
    written before this change hold as system messages. The instructions
    are placed as system text, so that a model which keeps system messages
    in place gets them as such.
  - The instruction texts are unchanged; no framing was added beyond what
    they say themselves ("Step 2 of 3: ...").
