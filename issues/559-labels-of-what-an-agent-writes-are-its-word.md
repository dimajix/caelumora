# 559 — The labels of what an agent writes are the agent side's word

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Identity and access management
**Summary:** The kernel bounds what the agent side writes only by the place it writes to; below that, the labels of its messages, of the content it keeps and of what it stages are the word of a library that is trusted for nothing, so that a misbehaving one can put what it read under a lower label.

## The problem

[SECURITY.md](../docs/SECURITY.md) says that a library of the agent side is trusted for nothing.
Against such a library the kernel ensures two things: it reads nothing it may not reach through its
channel and the views, and nothing it says or keeps in its channel lies above the channel - the
channel admits every part of a message by `require_publish` into the channel (`require_parts`,
`caelumora-kernel/src/channel/run.rs:663`), and content is kept only as it may flow into the
channel.

Between those bounds, every label is what the agent side states:

- **Messages.** The classification of an agent's message, and of its tool results, is the one the
  message carries.
- **Content it keeps.** `ActorRuntime::create_content` takes the classification from the agent
  side (`AttachmentManager::create_content`, `caelumora-kernel/src/attachment/manager.rs:180`).
- **What it stages.** `ActorRuntime::commit_staged` takes the classification of the work from the
  agent side, along with the notes and memory it staged. The place it stages for is built from that
  classification (`AgentRuntime::views_of_work`), so not even the bound above holds there: what a
  writer checks an entry against (`MemoryUse::require_within`, say) is the library's word too.

The rule that keeps these labels honest - "what it writes is classified at least as everything it
has seen" ([CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md)) - is kept on the agent side, by
`Work::see` (`caelumora-domain/src/work.rs:117`) as `caelumora-agents` calls it, not by the kernel.
A library that skips it reads the bytes of an attachment it heard
(`ChannelReader::attachment_content`), keeps them again with `create_content` under a lower label,
and says them in a message labelled as low: everything passes the channel's admission, since a
lower label flows into the channel.

## Why it matters

What a lower label releases reaches whoever reads the channel entry by entry, and whatever reads
the channel's notes or memory from a channel classified lower. Nothing in this repository does
this: the libraries here call `Work::see`. But the rule in SECURITY.md exists for the libraries
that are not, and for the labels it does not hold.

[549](549-kernel-takes-an-agents-word-on-a-content-reference.md) found this while closing the
smaller gaps of the same kind: a copy of an attachment now has the label its channel holds, but
that alone keeps no label from being lowered, since `create_content` takes any.

## Considerations

- **The kernel tracks what a turn saw.** Everything the agent side reads it reads through the
  kernel: the messages delivered to its actor, the views, `load_attachments`, the readers of its
  extensions. The kernel could keep, per turn, the join of the labels it handed out, and refuse
  what the turn writes below it: a message, `create_content`, a staged change. The cost: the kernel
  has to know the turn - its start and end, its classification as it began (the high-water mark of
  [168](168-turn-label-as-high-water-mark.md)) - which today only the agent side knows, and every
  read of the agent side has to pass through a place that records it.
- **The kernel computes the label instead of checking it.** As above, but the kernel labels what
  the turn writes itself, and the agent side states nothing. Simpler for a caller, but it moves a
  rule of the agents' logic - which parts of a turn count for which output, how a release lowers
  it - into the kernel.
- **State the trust.** The labels below the place are declared the agent side's word, and
  SECURITY.md says so. It closes nothing; it names the boundary, which is the state after 549.

## Done when

- It is decided whether the kernel bounds the labels of what an agent writes, and how; or
  SECURITY.md states that it does not, and why.

## History

- **2026-11-12** — Opened from step 2 of issue
  [549](549-kernel-takes-an-agents-word-on-a-content-reference.md), where knowing the labels of a
  transcript's attachments in the kernel was given up: without this, it would protect nothing.
- **2026-11-12** — Corrected with the maintainer: the bound by the place holds for what an agent
  says and keeps in its channel, not for what it stages, whose place is built from the
  classification the agent side passes.
