# 465 — Nothing bounds how much of its attachments a model is given along

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** What of a message's attachments is put into a model's context and what it has to fetch through a tool is decided by no bound, so a few images or one long text can fill the context and are sent again with every call.

## The problem

Issue [282](282-message-attachments.md) gives a model the attachments of the messages it hears in
two ways: given along - an image in the message it is attached to, a short text put into the
context - or fetched by the model itself through a tool (`view_attachment`, `read_attachment`),
with only a note in the context. Which way an attachment takes was left open there: 282 decided
that a large document is read through a tool and a small one given along, and that images are given
along to a model that can see, but not where the line lies, nor whether images are kept to it as
well.

## Why it matters

- **Every call sends it again.** The chat APIs are stateless (282, *What the model APIs accept*):
  what is given along is sent with every model call of every turn, until a compaction
  ([16](16-context-compression.md)) leaves it out.
- **An image is not cheap.** Ollama counts about 768 tokens an image when it truncates, OpenAI
  about 1 200 for 1024×1024 at `high`; a dozen screenshots in a channel are a large share of an 8k
  or 32k context.
- **Text is unbounded.** A pasted log or a CSV of some megabytes would leave no room for anything
  else.
- **Fetching costs too.** Every attachment fetched through a tool is a model call more and a step a
  small model may not take; what is obviously meant - "here is a screenshot, what is wrong?" -
  should be given along.

## Considerations

- **A fixed bound or a share of the context window.** A bound in bytes or tokens, configured, is
  simple but fits one model: what is harmless for a 128k model fills an 8k one. A share of the
  model's context window fits each model, but the window is not known everywhere: `ModelProfile`
  does not hold it; `ollama_client` knows it only where `context_size` (`num_ctx`) is configured.
- **Per attachment or per context.** A bound per attachment keeps one large file out; only a budget
  for all of them keeps many small ones from filling the context. A budget asks which are given
  along when not all fit - the newest, those of the message being answered.
- **Images.** Whether they are kept to the same bound, counted by an estimate of their tokens, or
  bounded by their number: the newest few given along, older ones only noted, to be fetched.
- **Compaction.** Whether a compaction leaves out what was given along and replaces it by its note
  is 16's; the bound here decides what is given along in the first place.

## Done when

- What of the attachments in a context is given along and what is only noted follows a bound that
  is documented, and configured where it is configurable.
- Images and text are both covered, and a model is never given more along than the bound allows.
- AGENT-TURN.md and CONFIGURATION.md describe it.

## History

- **2026-10-24** — Opened at the maintainer's request, moved out of issue 282, where it was the
  last question open (*The context window*): the threshold - configured, or a share of the model's
  context window - and whether images are kept to it as well.
