# 582 — An agent has no character of its own that it may develop

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** An agent is what its personality makes it, word for word, so it cannot develop a character, a manner or values of its own over time, and what it learns about itself is either lost or mixed into its memory of facts.

## The problem

What an agent is - how it speaks, what it values, how it goes about a task - is given by its
personality: the `system_prompt` component and the introduction, written by whoever may change the
personality and shared by every agent of it. The agent itself cannot change any of it, and that is
meant: the personality is a fixed property of an agent, configured by people.

But nothing lets an agent shape itself *within* that frame. What it learns about itself - that a
user prefers it terse, that it should ask before refactoring, that a joke landed badly - has
nowhere to go but its memory ([166](166-memory-as-topic-documents.md)), which holds what it knows
about the world, not who it is, and which is recalled only where each entry may be read.

Wanted, from the maintainer: a **soul** in the sense of the `SOUL.md` of some agent frameworks - one
document per agent, always in its context as a whole, that the agent revises itself, a section at a
time.

## Why it matters

- An agent that works with the same people for months should become better at it in a way that
  shows in every conversation, not only where a fact about it happens to be recalled.
- Instructions to itself and knowledge of the world are different things with different rules:
  instructions are in every prompt, everywhere; knowledge is recalled where it may be read. Mixing
  them makes both worse.

## Considerations

- **Fixed by the maintainer:**
  - The **personality stays** a property of the agent that the agent cannot change. The soul does
    not replace it: it is what the agent adds to it, given after the personality's own prompt and
    framed so that the personality prevails where the two disagree.
  - The soul is an **optional component** of an agent, wired by its personality like every other
    (see [COMPONENTS.md](../docs/COMPONENTS.md)). An agent whose personality does not list it has
    none.
  - It is **independent of the memory**, a component of its own in a crate of its own, not a
    reserved document of [166](166-memory-as-topic-documents.md).
- **The pattern to follow** is `caelumora-memory`
  ([518](518-memory-is-reached-through-the-agents-state.md)): a system service as a plugin and a
  library, one document per agent kept under the agent's id and
  owned by its owner, a reader bound to the work, changes staged on the turn and committed by the
  plugin under a lock per agent, removed when the agent is purged.
- **The text belongs to the agent, not to the personality.** Every agent of a personality starts
  from the same text - empty, or a seed given as a parameter of the component - and from then on its
  soul is its own. Whether a later change of the seed reaches agents that already have a soul is to
  be decided; the simple answer is no.
- **Where it stands in the context.** As a prompt part, whole, after the personality's system
  prompt: it changes rarely and is meant to frame everything. Every change breaks the prompt cache
  from there on, which is acceptable for something revised rarely, and a reason to keep it from
  being revised in every turn.
- **Confidentiality is the hard part.** The soul is given in every channel of every session the
  agent takes part in, so whatever is written into it is readable wherever the agent goes. A soul
  written from a confidential channel would carry what was said there into a public one. Options:
  - The soul has one fixed classification, the lowest; a change is taken only where the turn's
    label ([168](168-turn-label-as-high-water-mark.md)) flows into it. Safe and simple, but an agent
    working in a confidential channel could never revise its soul there.
  - A change from a turn whose label does not flow into the soul's classification is a release,
    and needs a user's approval as releasing a note or something remembered does
    ([APPROVALS.md](../docs/APPROVALS.md#what-needs-approval)).
  - The soul is shown only where its classification may be read. Then the agent would be someone
    else from channel to channel, which defeats the purpose.
  - Beyond the session the same holds for privacy ([134](134-session-privacy.md)): what is written
    from a private session reaches every other session.
- **A lasting injection.** Notes and memory are written without approval, accepted because a note
  is shown to every participant and every entry records where it was made. A soul is different: it
  is instructions to the model, in every prompt, from then on. A participant who talks the agent
  into revising its soul changes the agent for everyone, everywhere, until someone notices. That
  argues for every change needing its owner's approval, whatever the classification - or at least
  a change that adds instructions, if that can be told at all.
- **Who sees it.** Like memory, the agent's own and its owner's, or public like an introduction,
  since an agent shows its character in every answer anyway. Its owner should be able to read it,
  correct it and reset it to the seed - by the web UI and a command, as
  [574](574-notes-and-memory-cannot-be-read-by-a-command.md) wants for notes and memory.
- **Revising a section.** The soul is Markdown; options for the tool:
  - by section, named by its heading: `revise_soul(section, text)` replaces it, adds it where it
    is missing, removes it given no text. Matches "a paragraph at a time", and the model need not
    repeat what stays.
  - by search and replace (`old`, `new`), as a file-edit tool does: finer, proven with models, but
    fails where the text is not unique.
  - the whole text at once: simplest, but the model has to repeat everything.
  If [166](166-memory-as-topic-documents.md) chooses the same operation for memory documents, it is
  written once, as a function over a Markdown text in `caelumora-domain`, not in both crates.
- **Versions.** Whether every change is kept, as a ledger of the memory is, so that the owner can
  see how the soul came to be and go back - or only the current text with its revision. Kept
  changes cost little and are what an owner needs to find where an injected sentence came from.
- **Bounded.** The soul is paid for in every turn; a limit on its length, refused with a message
  the model can act on, keeps it short.
- **When it is revised.** During a turn, when the model thinks of it, or in a reflection after the
  turn ([55](55-notes-by-reflection.md)). The tool comes first; reflection is a later use of it.
- **Recording what was read.** The prompt part records the soul as knowledge, by its digest, as
  the memory's part does - private or not, depending on who sees it.
- **The name.** "Soul" is the word of the frameworks it comes from; "character" or "self" say the
  same with less pathos. One name, in the code and the documents.

## Done when

- An agent whose personality wires the component has a soul of its own: a Markdown document kept
  for it, given whole in its every context, revised by its own tool section by section, while the
  personality stays as configured.
- How a revision is classified, and whether and when it needs approval, is decided and enforced by
  the plugin as it commits it.
- Its owner can read, correct and reset it.
- A document in `docs/` describes it.

## History

- **2026-11-15** — Opened at the maintainer's request while [166](166-memory-as-topic-documents.md)
  was brought up to date: the personality stays a fixed property of the agent, the soul is an
  optional component of its own, independent of the memory and in a crate of its own.
