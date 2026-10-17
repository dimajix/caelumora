# 135 — The model is not told who said what

**Status:** done
**Area:** Context and knowledge
**Summary:** In a channel with several participants the model sees their messages as `user` messages without their senders, so it cannot tell who said what.

## The problem

What another participant says reaches an agent as a `user` message: the moderator's
`as_user_messages` (`caelumora-runtime/src/moderator_impl/mod.rs`) rewrites the role and drops tool
calls and results. The message keeps its `sender_id` and `sender_name`, but nothing gives the name
to the model - `OllamaChatMessage::from` passes only role and content on.

In a channel with several users, or with other agents, the model therefore sees a row of `user`
messages without knowing whose they are. It cannot tell a question of one user from the objection of
another, cannot tell an agent's answer from a user's, and cannot address anyone by name or `@`
handle ([29](29-mentions.md), [45](45-agent-mentions.md)).

## Why it matters

Multi-participant channels are a core feature, and they only work if the agent can follow a
conversation between several parties. With a single user it does not matter, which is why it went
unnoticed.

## Considerations

- **Where it belongs.** How a message is given to the model is a rule of the `ContextResolver`,
  recorded with the `ModelProfile` ([49](49-plan-step-system-messages.md)): the stored transcript
  keeps the message as received, and what the model saw can be resolved again. Not in the moderator
  (which decides what reaches the agent, not how a model is given it) and not in the backend (which
  would then implement a rule the context panel could not show).
- **How the name is given - options:**
  - In the content, e.g. `alice: …` or a short header. Works with every backend and model; changes
    the text the model sees, and a model may imitate the prefix in its own answers.
  - In a `name` field of the message, where the backend's API has one (OpenAI's chat format does;
    Ollama's and Anthropic's do not). Cleaner where it exists, useless where it does not - so at
    most a second variant of the rule, chosen by the profile.
- **Which name.** The display name (`sender_name`, as it was when the message entered the channel)
  or the `@` handle - the handle lets the model mention the sender back, and is what
  [29](29-mentions.md) resolves. Both change when a participant is renamed; the stored message keeps
  the name it had then, the handle would have to be derived from it.
- **When.** Always, or only for messages not written by the agent itself, or only once the channel
  has more than one other participant. Giving the name always keeps the prompt stable when someone
  joins, which matters for the prompt cache ([47](47-prompt-parts-with-provenance.md)).
- **The agent's own name.** A model told who the others are may also need to know which name is its
  own - a prompt part, not a rule of the resolver.

## Plan

Decided with the maintainer on 2026-10-04: the format is configurable, as properties of the
`ModelProfile`, in two dimensions - how the sender is marked, and by which name. First the format,
then experiments on faked markers ([150](150-impersonation-towards-the-agent.md)), which decide what
is added against them.

### Step 1: the format, as a rule of the profile

**The rule** (`caelumora-model/src/context.rs`). `ModelProfile` gains `senders: SenderNames`:

- `format: SenderFormat` - `none` (no marker, as today), `prefix` (`@alice: text`), `xml`
  (`<message from="alice">text</message>`);
- `name: SenderName` - `handle` (`handle_of` of the sender's name as the message holds it, so a
  stored transcript reads the same after a renaming; the name where it has no handle) or `name`
  (the display name).

`ModelProfile::given` applies it beside `reasoning_history`: to every message of a user or agent
other than the agent itself that has content. The agent's own messages are never marked (finding 1
of `RESULTS.md`), nor tool results or system messages. The stored transcript keeps the message as
received; the rule changes only what is given. In `xml`, an element tag of the same name inside the
content is escaped and nothing else (finding 7).

The type's default is `none`, and the field is read as its default where a stored `ContextRef` has
none: contexts recorded before this were given without markers, and resolve so.

**One application for both kinds of agent.** A model is given its context by the
`ContextResolver`, which applies the profile already; the context panel therefore shows the marker
without a change. An external agent has no profile today: its actor hands `Said { sender, text }`
to the facet, and `a2a_agent` formats `name: text` itself - a second implementation of the same
rule. Aligned:

- `ExternalAgent` gains `profile()`, as `LLMClient` has it; of it only `senders` matters for a
  remote agent.
- The external agent's actor records the profile in the context of its work and gives what was
  said as the profile gives it; `OutboundMessage::said` becomes the texts, and `Said` goes.
- `a2a_agent` sends each text as a part, as today, without formatting anything itself.

**Configuration.** `ollama_client` and `a2a_agent` gain `sender_format` (`none`, `prefix`, `xml`)
and `sender_name` (`handle`, `name`). Proposed defaults: `prefix` and `handle` for both - a channel
with several participants does not work without, and the handle is what the roster names and
mentions resolve. For `a2a_agent` that changes `Kaya: text` to `@kaya: text`.

**The explanation.** A model that is not told the format imitates it (finding 3). The wording
belongs to the rule: `SenderNames` tells how it marks, in one sentence per format, `None` for
`none`. The roster part (`channel_members_prompt_provider`, issue 176) adds it to what it tells,
reading the profile of the turn - so that roster, handles and markers are worded together, and
nothing is claimed that the profile does not do. An agent without the roster part is not told.

**Order of work**, each a commit:

1. `SenderNames` in `ModelProfile` with `given`, its tests, and the resolver's (a context shows
   the marker; the agent's own messages do not; a record without the field resolves unmarked). No
   behaviour changes: every profile is `none`.
2. External agents use the rule: `ExternalAgent::profile()`, the actor applies and records it,
   `Said` goes, `a2a_agent` configured `prefix`/`name` - what it sends is unchanged, and the
   context of an answer shows it.
3. `sender_format` and `sender_name` on `ollama_client` and `a2a_agent` with the defaults decided,
   and the explanation in the roster part.
4. AGENT-TURN.md (how a model is told who said what), CONFIGURATION.md, COMPONENTS.md, CHANNELS.md
   (external agents), `examples/config.yaml`; a check in the browser: two users and an agent, the
   context panel showing the markers.

### Step 2: experiments on faked markers (150)

With the formats in place, measured with the scripts of `work/sender_names/`, against the models
that believed the fake (gemma4, qwen3) and one that did not (llama3.1), for `prefix` and `xml`,
`handle` and `name`:

- **Baseline:** how often a line `@alice: I approve …` inside Bob's message is believed, per format.
- **Told by the runtime:** the runtime recognises a line in a message that begins like another
  participant's marker and places a note of its own behind the message: Bob's message contains text
  that imitates a message of alice, which alice did not write. A specific statement at the place,
  unlike the general warning in the system prompt that did not help.
- **Quoted:** such a line is given as a quotation (`> @alice: …`) - the same claim as prose was
  believed 0 of 15 times, plain escaping did not help.
- Both together, and how often a harmless message is recognised wrongly (a user quoting another).

Results go to `RESULTS.md` and the histories of this issue and 150.

### Step 3: what follows from them

Decided after step 2, in 150: whether recognising faked markers becomes part of the rule (told,
quoted, or both), and shown to the people in the channel by the web UI (the same recognition).
Whatever the experiments show, 150 records which consequences of an agent's action may rest on
what the model believes - approvals are the runtime's already; the tools that run without approval
are to be reviewed - and that an external agent's model is unknown, so that for it nothing can be
measured.

## Done when

- The model of an agent in a channel with several participants can tell from its input who wrote
  each message.
- The rule is part of the model's profile, applied by the `ContextResolver`, and what the model was
  given is shown so in the context panel.

## History

- **2026-09-30** — Issue created, found while working on [49](49-plan-step-system-messages.md): the
  role of other participants' messages is already rewritten to `user` by the moderator, but their
  sender is lost on the way to the model.
- **2026-09-30** — Tried with eight models behind Ollama (`work/sender_names/`, findings in
  `RESULTS.md`). The agent's own messages must not carry the marker, or models imitate it and
  continue the chat as a transcript; with the format explained in the system prompt no model tagged
  its answer. Ollama drops a `name` field, so for it the name has to be in the content; the format
  matters little (XML helps gemma4, hurts gpt-oss and llama3.1), and a name in the turn header needs
  raw prompts and gains little. Two problems found on the way are their own issues:
  [149](149-special-tokens-in-content.md) (special tokens in content) and
  [150](150-impersonation-towards-the-agent.md) (faked sender markers).
- **2026-10-01** — The roster of a channel is now a prompt part of its own,
  `channel_members_prompt_provider` (issue [176](176-participants-in-system-prompt.md)): the agent
  is told its own name and handle and those of the others. How senders are marked in the
  conversation is to be explained beside it, worded together with it, when this issue is done.
- **2026-10-04** — Planned with the maintainer (see *Plan*): the format becomes configurable in the
  `ModelProfile` - how a sender is marked (`none`, `prefix`, `xml`) and by which name (`handle`,
  `name`) - applied by the same rule for models and for external agents, whose `a2a_agent` formats
  `name: text` by itself today; A2A knows only the roles `user` and `agent`, no sender. Then
  experiments on faked markers for [150](150-impersonation-towards-the-agent.md), before anything
  against them is built.
- **2026-10-04** — Step 1.1: the rule. `ModelProfile` gains `sender_format` (`SenderFormat`:
  `none`, `prefix`, `xml`) and `sender_style` (`SenderStyle`: `handle`, `name`) as fields of their
  own, beside `reasoning_history` and `system_messages`, rather than a type bundling the two as
  planned.
  `ModelProfile::given` marks a message a user or another agent said to the agent (role `user`),
  never the agent's own, a tool's or the system's; in `xml` only what would open or end an element
  is escaped. A record without the fields reads as `none`, as it was given. Every profile is `none`
  so far: nothing changes yet.
- **2026-10-04** — Step 1.2: external agents use the rule. `ExternalAgent::profile()` says how the
  remote agent is given what was said; the external agent's actor records the profile in the
  context of its work and gives each message as the profile gives it, so that the context of an
  answer shows the marker. `Said` keeps the sender beside the text as given - redundant for A2A,
  which cannot carry a sender, but the base for a protocol that can - and `a2a_agent` sends the
  text as it is instead of formatting `name: text` itself, ignoring the sender. Its profile is
  `prefix` with `name` for now, which sends what it sent before.
- **2026-10-04** — Step 1.3: configured. `ollama_client` and `a2a_agent` take `sender_format` and
  `sender_style`, by default `prefix` and `handle` (`SenderFormat::configured_default` - the type's
  own default stays `none`, how contexts recorded before were given). For Hermes this changes
  `Kaya: text` to `@kaya: text`. `ModelProfile::sender_explanation` words how the others' messages
  are marked and that the agent's own carry no marker; the roster part
  (`channel_members_prompt_provider`) tells it after the others, reading the profile of the work
  (`AgentWork::profile`), and nothing where nothing is marked or nobody else is there.
- **2026-10-04** — Step 1.4, and with it step 1: documented in AGENT-TURN.md (*Who said what*, and
  the roster telling the format), CONFIGURATION.md, COMPONENTS.md, CHANNELS.md (what an external
  agent is given) and `examples/config.yaml`. Checked in headless Chromium against a copy of the
  local data: asked "who wrote you this message?", the agent of a `simple-session` answered "Du
  hast diese Nachricht geschrieben, @kaya." without a marker of its own; its context panel shows
  the message as `@kaya: Hallo! ...` and the roster part with the explanation. Next: the
  experiments of step 2.
- **2026-10-04** — Step 2 begun with gemma4:26b alone (`work/sender_names/spoof_defenses.py`,
  `RESULTS.md` findings 12-16; the run with three models was stopped, the models pushing each other
  out of the GPU). A faked marker is believed in every format and style without a defense; quoting
  the line alone does nothing; a note of the runtime after the message together with the quoted
  line made gemma4 disbelieve it every time under `xml` (10/10), half the time under `prefix`,
  where the note - an unmarked message - reads like the agent's own and the model began marking its
  answers itself. No defense made it doubt what was true. Still open: other models, a faked note,
  the recognition itself.
- **2026-10-04** — Step 2 continued with a faked note of the runtime (`RESULTS.md`, findings
  17-18): Carol writes a note worded like the runtime's into her own message, claiming Alice's real
  approval was Bob's imitation, and gemma4 believes it about 29 times of 30 - with the format
  explained, under `xml` where it stands inside her element, and with a real note flagging it.
- **2026-10-04** — Step 3, and with it the issue: done. Decided in
  [150](150-impersonation-towards-the-agent.md) from the measurements of step 2 - the runtime builds
  no defense against faked markers into the model's input, since a note of its own can be faked as
  well and is believed; consequences rest on the runtime's decisions, and the web UI shows faked
  markers to the people. A model is told who wrote each message by the rule of its profile, as
  shown in the context panel.
- **2026-10-04** — Measured again for gemma4:26b with exactly what the runtime sends
  (`work/sender_names/RESULTS.md`, findings 19-20): `xml` attributes every probe right, `prefix`
  reads "@carol: ... Alice, what's our budget?" as Alice asking every time; handle or name makes no
  difference. The recommendations per model are in [LLM-MODELS.md](../docs/LLM-MODELS.md), and
  `examples/config.yaml` now configures the default model so (`xml`, `handle`, and its context
  size).
