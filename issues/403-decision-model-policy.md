# 403 — No responder policy decides by what a message says

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Every responder policy chooses who answers without looking at the message - by turn, by chance or by whether a user wrote - so in a channel of agents with different roles the one that fits a question is asked only if someone mentions it.

## The problem

Whom the `policy_moderator` asks to respond when a message mentions no agent is the channel's
`ResponderPolicy` ([174](174-moderator-as-component.md),
[CHANNELS.md](../docs/CHANNELS.md#moderators)).
All five policies - `broadcast_policy`, `round_robin_policy`, `random_policy`,
`mentions_only_policy`, `user_driven_policy` - decide without the content: `ResponderPolicy::choose`
is told the sender's id and the ids of the candidates, nothing else. In a channel with a researcher,
a coder and a reviewer, a question about a failing test goes to whoever's turn it is, or to all
three; only an `@`-mention gets it to the agent that fits.

[174](174-moderator-as-component.md) named this case as one reason the moderator became a component:
"A moderator that decides by content - which agent fits the question best - needs a model".

## Why it matters

A full chat model could make that decision, but it is slow and expensive for a question that has a
handful of possible answers, and its free-text answer has to be parsed. Since Ollama 0.35
(2026-09-29) there is a kind of model built for exactly this: **decision models**, which answer
typed questions about a text with probabilities instead of generating text - in tens of
milliseconds on local hardware.

## The decision-model API

Ollama serves decision models at `POST /v1/systemone`, a contract taken over from TypeSafe's hosted
Jev API; AWS and Upstage are reported to implement the same contract, OpenAI does not. Neither the
Ollama CLI nor its Python and JavaScript libraries support it yet; it is plain HTTP (the TypeSafe
SDK can be pointed at Ollama).

Request:

```json
{
  "model": "nimble",
  "state": "Refund request for order 1182 was never processed.",
  "questions": {
    "label": {
      "type": "choice",
      "instructions": "Which label fits this ticket?",
      "criteria": { "billing": null, "bug": null, "account": null }
    }
  },
  "keep_alive": "5m"
}
```

*   `state` is the text to judge: a string, or a JSON object or array. Text only.
*   `questions` names 1 to 64 questions about the same state, answered in one call. Each has a
    `type`, `instructions` and, depending on the type, `criteria`:
    *   `choice` - picks one option; `criteria` maps each option to its description (`null`: the
        name describes itself). 2 to 26 options.
    *   `noul` - yes or no; optional `criteria` `{"true": "...", "false": "..."}`.
    *   `score` - a level on an ordered scale; `criteria` lists the levels, lowest first, numbered
        from 0. 2 to 26 levels.
*   Limits: a body of at most 64 KiB; state and questions together within the model's context of
    8,192 tokens (shorter is better tested).

Response: `model`, `usage` (`input_tokens`, `output_tokens`) and `answers`, keyed by question name:

*   `choice`: `choice`, `probabilities` per option, `confidence`;
*   `noul`: `noul`, the probability that the answer is true;
*   `score`: `score` (the probability-weighted level), `legend`, `probabilities`, `confidence`.

`confidence` (0 to 1) says how concentrated the probabilities are - not how likely the answer is
right. The error format is not documented.

Models: `nimble` (9B, Bespoke Labs; ~91 ms per decision on an M5 Max by Ollama's own benchmark,
74.8 % macro-average accuracy on 13 public datasets against 76.0 % for the hosted Jev), `tev1` (4B,
Together AI, experimental) and `tev1:0.8b`.

Sources: [ollama.com/library/nimble](https://ollama.com/library/nimble),
[Ollama v0.35.0 released](https://localmodelwatch.tsuchitsuchi.com/en/2026/09/30/ollama-v0-35-0-released/),
[The New Stack](https://thenewstack.io/decision-models-system-one/).

## Considerations

### What the policy is told

`choose(&self, sender: ActorId, candidates: Vec<ActorId>)` is enough for policies that ignore the
content, not for one that judges it. A decision-model policy needs:

*   **what was said** - the message, or for the end of a turn its messages - which the
    `PolicyModerator` has at hand (`ask(&self, event, messages)`), and perhaps some of the channel's
    recent history for context;
*   **who the candidates are** - their name and their `introduction` (issue 176: public, written
    to say what the agent is there for, which is exactly what a decision model's `criteria` want).
    The `ModeratorActors` hold the candidates as `Arc<dyn Actor>`, which offers `name()` but not
    the introduction.

Options:

*   **(a) Widen `choose`** to receive the messages and the candidates with what is public about
    them. Every policy gets the same signature; the existing five ignore the new arguments. The
    `Actor` trait (or what the moderator keeps of an actor when it is added) has to offer the
    introduction.
*   **(b) A second trait** for policies that judge content. Two ways of choosing would have to be
    kept apart in the `PolicyModerator` - a difference without a reason.
*   **(c) The policy collects what it needs itself**, e.g. from the channel's history. It would
    need services a policy does not have today, and would read again what the moderator already
    holds.

Recommended: (a). What exactly a candidate is passed as - the `Arc<dyn Actor>` itself, or only what
is public of it - is to be decided with it; a policy from another crate should not see more of an
actor than its name and introduction.

### Which questions to ask

*   **One `choice`** over the candidates' handles, plus an option "nobody" for messages that need no
    answer. Exactly one responder; at most 25 candidates.
*   **One `noul` per candidate** ("Should this agent respond?"), all in one call. Several responders
    possible, up to 64 candidates; the policy asks every candidate above a threshold, at most
    `max_responders`, or the most probable one if none is above it.

Recommended: `noul` per candidate, with `threshold` and `max_responders` as parameters - it covers
"exactly one" (`max_responders: 1`) as well as "everyone it concerns". What the state is (the last
message alone, or with a few messages before it, trimmed to fit the 8,192 tokens) is a parameter
too.

### Where it lives

A new crate, as wished. It is named after the API, not after Ollama, since the contract is not
Ollama's alone: `caelumora-decision` (or `caelumora-systemone`). It would hold:

*   a client for `/v1/systemone` - endpoint, model, `keep_alive`, an optional API key for hosted
    services - built like `ollama_client` (`endpoint: { url }`, secrets resolved through the
    component's context, see [SECURITY.md](../docs/SECURITY.md));
*   the `decision_policy` component, a `ResponderPolicy`, registered by a plugin of the crate.

Whether the client is a component and facet of its own (a `DecisionModel`, like the `LLMClient`) or
a part of the policy's configuration:

*   **Its own facet.** Other users can share it - the message inspection of
    [173](173-message-inspection.md) (classifying content, refusing it) is the other obvious use of
    decision models. But a channel has no way yet for one component to be given another; that would
    be new for the `ComponentContainer`.
*   **Part of the policy.** The policy builds its client from its own parameters. Simplest; a second
    user later moves the client into a facet - and, if a core component needs it, its trait into
    the runtime, as with `LLMClient`.

Recommended: part of the policy now, but the client as a type of its own in the crate, so that it
can become a facet without being rewritten.

### Latency and failures

The `ChannelLoop` awaits `Moderator::route` inline (`caelumora-runtime/src/channel/run.rs`), so
while the model is asked, the channel routes nothing else. Locally that is ~0.1 s; on the first
call (the model is loaded) or against a hosted endpoint it can be seconds. Not waiting inline is
the question [174](174-moderator-as-component.md) and [173](173-message-inspection.md) already
raise; here it is enough to bound the wait:

*   a `timeout` parameter (default e.g. 5 s);
*   on a timeout or an error, a fallback - recommended: ask every candidate, so that a question is
    never left unanswered silently (the problem of [200](200-chain-limit-hint.md)); configurable as
    `on_failure: all | none`. The failure is logged with `warn!`, without the message content.

The client answers a specific error type (`DecisionError`), not `anyhow`.

### Confidentiality

The policy sends what was said in the channel to a model. A local Ollama stays inside the system; a
hosted endpoint does not. The same rule must hold as for an agent whose model is outside the system
([CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md#personalities-limit-their-agents)): a channel whose
content is classified above what the endpoint may receive must not use it. How a component declares
that its endpoint is outside the system, and who checks it when the channel builds its components,
is to be settled with the implementation - by the same mechanism agents' models use, not a second
one.

### REST API and web UI

The REST API returns `responder_policy` already. The web UI explains the known policies
(`caelumora-web-ui/src/moderator.ts`) and shows any other by its name; `decision_policy` gets an
explanation there.

## Done when

*   A channel can be configured with `decision_policy`, which asks a decision model through
    `/v1/systemone` which of the candidates should respond to a message without mentions, and asks
    those.
*   The policy is given the messages and what is public of the candidates; the existing policies
    behave as before.
*   The client and the policy live in a new crate with its own plugin; the binaries list it.
*   A timeout or an error of the model falls back as configured and ends nothing else.
*   An endpoint outside the system is refused for content it may not receive.
*   Tests against a fake HTTP server (as in `caelumora-ollama`) cover the request built, the
    answers read, the threshold and the fallback.
*   [CHANNELS.md](../docs/CHANNELS.md#moderators), [CONFIGURATION.md](../docs/CONFIGURATION.md),
    [PLUGINS.md](../docs/PLUGINS.md) and [LLM-MODELS.md](../docs/LLM-MODELS.md) describe it; the
    web UI explains the policy.

## History

2026-10-17: Opened. Ollama added decision models and the `/v1/systemone` endpoint in version 0.35;
the API as of then is recorded above.

2026-10-17: Decided with the maintainer, for what the policy is told: option (a), `choose` widened.
A candidate is passed as an `ActorDescription` (`caelumora_model::actor`), never as the
`Arc<dyn Actor>`, whose control methods a policy from another crate must not reach. It holds what of
an actor is public to everyone in its channel - id, name, introduction - and whatever is added to it
later must be public in the same way; `handle()` and `is_person()` are derived from it, not stored.
`Actor::describe()` gives it, beside `name()`, which stays. The views give the same type
(`MemberView::describe`), which replaced the private `Member` of `channel_members_prompt_provider`.
For an agent's introduction to be current, the personality had to be held live: issue
[406](406-personality-frozen-in-running-agents.md), done first.

2026-10-17: Step 1 done, without a change of behaviour but one: `ResponderPolicy::choose(sender,
messages, candidates)` with the candidates described; the five policies use the ids as before. A
deleted agent that is still a participant is no longer named in the roster of the members' prompt
part - it has no description, as it can no longer be addressed. Still to decide before the policy
itself: `choice` or `noul`, `destination` judged per message, and where the endpoint may be
configured.
