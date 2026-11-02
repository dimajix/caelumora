# 529 — The model's profile resolves a transcript beside the context's resolver

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Context and knowledge
**Summary:** `ModelProfile::given_messages` resolves what an external agent's remote agent is given of a transcript - the messages as they stand, a notice where someone joined or left, each framed - beside `ContextResolver`, which resolves what a model is given, so that the profile depends on the transcript and the notices of a channel are made in two files.

## The problem

Since issue [526](526-large-modules-of-the-domain.md) the context of a model lies in
`caelumora-domain/src/context/`: what a context is made of (`item.rs`), how a model is to be given
it (`profile.rs`, `ModelProfile`) and how a reference is resolved into what it is given
(`resolver.rs`, `ContextResolver`). One resolution lies apart from the others:
`ModelProfile::given_messages`, which an external agent calls
(`caelumora-agents/src/actor/external_agent.rs`) to give its remote agent the transcript. It walks
the transcript's messages as they stand (`as_they_stand`), adds what is told where someone joined or
left (`participant_notice`) and frames each by the profile (`given`) - what
`ContextResolver::given_path` does for a model run here.

Because of it, `profile.rs` imports the transcript (`Transcript`, `as_they_stand`,
`TranscriptEntry`), `participant_notice` is `pub(super)` in `profile.rs` instead of private in
`resolver.rs` beside its sibling `given_up_notice`, and the test of `given_messages` lies in
`resolver.rs`, apart from the method it tests.

Found in the review of the step `context` of issue 526.

## Whether the two resolutions differ

Looked at on 2026-11-02, before deciding: they give the same, for the transcript an external agent
keeps. `given_path` does three things `given_messages` does not, none of which applies there:

- **A turn given up** is told where it stands (`given_up_notice`). An external agent records no
  `TurnGivenUp` in its transcript: only a turn of an agent run here does (`give_up_turn`, called by
  `turn::ending`).
- **The messages of a turn given up because the channel was lowered** are given as the tools they
  called alone (`Message::names_only`, issue 141). An external agent's transcript holds what it
  heard of others, not its own answers, and no turn of its own is given up in it.
- **The prompt parts** are placed where they were. An external agent sends them apart
  (`OutboundMessage::prompts`).

One difference remains: the resolver places system messages as the profile says
(`ModelProfile::place_system_messages`); `given_messages` does not, so a notice given as a system
message stays one wherever it stands. Whether a remote agent should be given them so is a question
of the external agents' protocol, not of where the method lies.

## Considerations

- **A method of the resolver.** `ContextResolver::of(state, catalogue)` holds the transcript and the
  catalogue; `given_messages(&self, profile, agent, time_zone)` there takes the texts from the
  profile itself (`profile.texts(catalogue)`), so its caller passes one parameter less.
  `participant_notice` becomes private in `resolver.rs`, `profile.rs` no longer imports the
  transcript, and the test lies beside the method. The path `ModelProfile::given_messages` changes;
  its one caller is the external agent.
- **A function of `resolver.rs`** instead of a method: the same cut, but a second way of resolving
  beside the resolver's own, which takes what it holds as parameters.

Recommended: a method of the resolver, without changing what it gives; the placing of system
messages, if wanted, as a change of its own.

## Done when

- What an external agent's remote agent is given of a transcript is resolved by `ContextResolver`,
  `profile.rs` depends on no transcript, and `participant_notice` is private to `resolver.rs`.

## History

- **2026-11-02** — Opened at the maintainer's request, from the review of the step `context` of
  issue [526](526-large-modules-of-the-domain.md), after looking at whether the two resolutions
  differ.
