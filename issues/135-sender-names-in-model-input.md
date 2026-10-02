# 135 — The model is not told who said what

**Status:** open
**Area:** Context & knowledge

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
