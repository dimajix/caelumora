# 401 — A message cannot address everyone in the channel at once

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** A user who wants every participant of a channel to respond has to `@`-mention each of them by name; there is no `@all` or `@channel` that addresses them all at once.

## The problem

An `@`-mention addresses one participant by its handle (issue [29](29-mentions.md)): the channel
resolves the handles in a message to actor ids as it enters (`MentionDirectory::resolve` in
`caelumora_model::mention`), stores them in `Message::mentions`, and every moderator asks exactly
the mentioned agents to respond instead of applying its policy (`mentioned_responders` in
`caelumora-runtime/src/moderator/policy_moderator.rs`).

What is missing is a way to address everyone at once. Under `round_robin_policy`, `random_policy`
or `mentions_only_policy` a user who wants every agent's view on a question - "@all, what do you
think?" - has to write out every handle, and has to know who is in the channel to do so. In a
channel with many agents that is tedious, and an agent added later is easily left out. Switching
the channel to `broadcast_policy` for one question changes it for every message after it.

## Why it matters

Asking all participants is a common move in a group conversation - a round of opinions, a vote, a
status question - and the chat tools users know offer it as `@channel`, `@all` or `@everyone`.
Without it, the mention is the one way to choose who answers, but only one at a time.

## Considerations

**Where it is resolved.** Two places come into question:

- *As the mention is resolved, to the participants of the moment.* `MentionDirectory` knows the
  group handles `all` and `channel` and resolves them to every participant it holds; what is stored
  in `Message::mentions` is the ids, as for any mention. The moderator needs no change: it asks
  every mentioned agent but the sender, as it does today. It keeps the rule of issue 29 that
  mentions are resolved once - a participant who joins later was not addressed by a past `@all`,
  and none of the readers of `Message::mentions` has to know about groups.
- *As a marker on the message, expanded by the moderator.* `Message` would say "mentions everyone",
  and each moderator would turn it into the agents of the moment. Every reader of `mentions` - the
  moderator, the web UI's highlighting, the agents' transcripts - would have to know the marker,
  and what a past message addressed would depend on when it is read. Rejected for that reason.

Recommendation: the first. The group handles are part of the rules of mentions and belong to
`caelumora_model::mention` with them, not to the runtime.

**Who "everyone" is.** Every participant that can be mentioned - users and agents, not approvers
(`ChannelLoop::is_mentionable`). Asking to respond concerns only agents, so what changes in effect
is that every agent but the sender is asked; users are only marked as mentioned, as today. Agents
that are asleep are woken first, as for every mention (see `docs/CHANNELS.md`, *Waking*) - `@all`
is a deliberate address of everyone, so that is the expected effect, not a side effect.

**`@all` and `@channel`.** The two are synonyms. Slack's `@here` - only those who are present -
would correspond to "only the agents that are awake"; it is a different question, left out here
and worth an issue of its own once someone needs it.

**A participant whose handle is `all` or `channel`.** An agent called "All" or "Channel" has that
handle. Options:

- *Refuse such names.* Names are not local to a channel, so this would restrict every agent and
  user for the sake of one rule of channels.
- *The participant wins.* `@all` would then mean "everyone" in one channel and "that agent" in
  another, depending on who is present.
- *The group wins.* `@all` always means everyone; the participant called "All" is among them, so it
  is still addressed, just not alone. The handle is reported like an ambiguous one
  (`MentionDirectory::ambiguous`, and the warning of `ChannelLoop::rebuild_mentions`).

Recommendation: the group wins. It keeps one meaning of `@all` everywhere and restricts no name.

**Agents writing `@all`.** Mentions by agents are routed like those of users (issue
[45](45-agent-mentions.md)), and `@all` should be no exception. It fans out: each agent asked may
answer with `@all` again. The depth is bounded by `max_agent_hops` (issue
[200](200-chain-limit-hint.md)) exactly as under `broadcast_policy`, which already makes every
agent answer every agent; nothing new is needed for it. Whether the agents' prompt names `@all`
(`ChannelMembersPromptProvider`, and the roster of an external agent, issue
[204](204-external-agent-roster.md)) is part of the decision: naming it invites its use;
recommendation: name it, so that agents read `@all` in a user's message as addressed to them too.

**Permission.** None of its own. Whoever may write in a channel may mention anyone in it, and
`@all` asks nothing that mentioning each participant by hand would not (as `/nudge` reasons, issue
[278](278-nudge-command.md)). Chat tools restrict `@channel` in large channels because it notifies
people; here it asks agents to respond, which is bounded by the channel's own limits. If that ever
changes, the restriction is a question of `AccessQuestions`, not of the mention.

**Questions of an agent** (issue [165](165-ask-the-user-tool.md)). `Questions::resolve` looks a
handle up in the same `MentionDirectory`, filtered to users. If the group handles live in `lookup`,
an agent's question to `@all` is put to every user in the channel - a consequence of the
recommended place, and one that fits; it should be stated and tested rather than left to happen.

**`/nudge @all`.** `/nudge` names one participant by name or handle (`participants_named`), not by
a mention. Nudging everyone is conceivable, but is not this issue.

**The web UI.** `caelumora-web-ui/src/mention.ts` mirrors `caelumora_model::mention`: it has to
know the group handles to highlight `@all` and `@channel` in a message, and the completion offers
both alongside the participants' handles. The TUI has no completion of mentions and needs nothing.

**Stored data.** `Message::mentions` keeps holding actor ids; nothing stored changes form.

## Done when

- `@all` and `@channel` in a message, outside code, resolve to every mentionable participant of the
  channel at the moment the message enters it; the ids are stored in `Message::mentions`.
- Every moderator asks every agent but the sender to respond to such a message, under every policy;
  sleeping agents are woken first; a chain between agents is cut off at `max_agent_hops` as for any
  mention.
- A participant whose handle is `all` or `channel` is included in the group and reported as an
  ambiguous handle.
- An agent's question to `@all` is put to every user of the channel.
- The agents' prompt and an external agent's roster name `@all`.
- The web UI highlights `@all` and `@channel` and offers them in its completion.
- `docs/CHANNELS.md` (*`@`-Mentions*) describes the group handles.

## History

- 2026-10-17: Opened at the maintainer's request: all participants of a channel should be
  addressable by `@all` or `@channel`.
