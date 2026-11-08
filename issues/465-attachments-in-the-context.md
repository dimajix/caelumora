# 465 — Nothing bounds how much of its attachments a model is given along

**Status:** done
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

## Decision

Decided with the maintainer: a bound by number, per model. It needs no context size, its cost can
be told in advance, and a small model is simply given a smaller number. A text stays noted alone,
as it is: nothing of it is given along that a bound would have to cover.

```rust
// ModelProfile, in place of `images: bool`
/// How many images the model is given along at most, the newest of its context; 0: it sees none.
/// Read as 0 where a context recorded before has none.
#[serde(default)]
pub max_images: u32,

impl ModelProfile {
    /// Whether the model sees images at all.
    pub fn sees_images(&self) -> bool;
    /// Marks the images of `messages`, oldest first, beyond the newest `max_images` as left out.
    pub(super) fn leave_out_images<'m>(
        &self,
        messages: impl DoubleEndedIterator<Item = &'m mut Message>,
    );
}

// ContentRef, as `number` and `content` on the copy alone, never stored
/// Whether the model is told of it alone, beyond the images it is given along.
#[serde(skip)]
pub left_out: bool,

// LanguageModelParams (component `language_model`)
/// How many images the model is given along at most, if it sees any.
#[serde(default = "default_max_images")]
max_images: u32,
```

- `ContextResolver::given_path` marks once, before anything is worded, on what `resolve`,
  `conversation_until` and the estimate of tokens all read. `gives_along` asks `!left_out` too, so
  that the note, the estimate, the label and what is loaded cannot disagree.
- What counts is what the model would be given: a message of a turn given up for a lowered channel,
  reduced to the tools it called, carries none. Within a message the last image counts first.
- `view_attachment` shows an image as the newest, so that one left out comes back.
- A result of `view_attachment` whose image was left out is noted as not shown: its value still
  says `shown`, and a model told of an image it is not given makes one up.
- 4 by default, some 3,000 tokens; a small context is given a smaller number.

The price: once a new image pushes an old one out, the old one's note changes far ahead in the
context, as a compaction changes it, which may invalidate a model's prompt cache from there on.

## Done when

- A model is given along at most `max_images` images, the newest of its context, and is told of the
  others alone, in its note, its estimate, its label and what is loaded.
- `language_model` configures it, with a default.
- AGENT-TURN.md, CONFIGURATION.md, COMPONENTS.md and LLM-MODELS.md describe it.

## History

- **2026-10-24** — Opened at the maintainer's request, moved out of issue 282, where it was the
  last question open (*The context window*): the threshold - configured, or a share of the model's
  context window - and whether images are kept to it as well.
- **2026-11-06** — The same question for what a tool gives back, found in the comparison of Claude
  Code's session logs that led to issue [537](537-prompt-parts-placed-anew-every-turn.md): a tool
  result larger than `max_output_bytes` of `workspace` is cut off, where Claude Code keeps it
  whole apart and gives the model a preview with a reference to fetch the rest
  (`<persisted-output>`). A large result could be an attachment of the tool result, given along
  or fetched by the same bound - to be decided with it.
- **2026-11-08** — Split with the maintainer: what a tool gives back is issue
  [552](552-a-tool-result-has-no-bound.md), this issue keeps what of its attachments a model is
  given along. Checked against the code: a text is never given along (`ModelProfile::gives_along`),
  so only images are left to bound, and the context size is known after an agent's first call where
  its backend reports it.
- **2026-11-08** — Decided with the maintainer as above: a number per model, 4 by default.
- **2026-11-08** — Implemented as decided.
