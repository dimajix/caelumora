# 551 — A decision model is asked past the models the platform offers

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** The client of a decision model lives in the crate of the `decision_policy` that uses it and is configured by the policy's parameters, while language and image models are offered by `caelumora-model` and served by backends of their own, so no call of a decision model passes that plugin and nothing but the policy can ask one.

## The problem

Language and image models follow one pattern (issues [520](520-the-kernel-sees-no-model-call.md),
525): the interface of a backend lies in `caelumora-model` (`LanguageModelProvider`,
`ImageModelProvider`), the implementation in a crate of its own (`caelumora-model-ollama`,
`caelumora-model-comfyui`), the models are named in `plugins.model`, and the agent side asks one
through a facet of `caelumora-api`, on a client the `ModelManager` builds for whoever asks.

A decision model follows none of it. `DecisionClient` (`caelumora-decision/src/client.rs`) is a part
of the crate of `decision_policy`, as issue [403](403-decision-model-policy.md) recommended until a
second user should appear, and where its server is, which model it asks and what that server is
cleared for are parameters of the policy.

## Why it matters

- **"Who asks which model" has one place, and this passes by it.** `caelumora-model` is "the one
  place where what is called of the models the platform offers, by whom, can be known".
- **A second user is waiting.** Issue [550](550-a-skill-is-an-entity-alone.md) wants a decision
  model to judge which skill fits. The library of `skills` may not link `caelumora-decision`, which
  links the kernel, and a second configuration of the same server would be a second way of doing the
  same thing.
- **The server is configured per channel.** Every template that uses the policy repeats where the
  server is and what it may be told.

## Considerations

Where the implementation goes:

- **A. As a further kind of the backend `ollama`, in `caelumora-model-ollama`.** No new crate. But
  `/v1/systemone` is a contract that hosted services implement too, which is why issue 403 named
  the crate after it, and that backend refuses `endpoint.auth`, which `decision_policy` offers
  today: the refactoring would take something away.
- **B. A crate of its own, `caelumora-model-systemone`, with the backend `systemone`** (decided).
  It is named after the contract as `caelumora-model-comfyui` is after what it reaches, keeps the
  credential, and Ollama is simply one server that serves the contract, by its URL.
- **C. Left in `caelumora-decision`, beside the policy.** That crate would be a backend and a
  policy at once, which belong together only by the word "decision".

## The plan

Proposed for the maintainer's review; the patterns followed are `image_model` and
`ImageModelClient`, and `caelumora-model-comfyui` for the backend.

```rust
// caelumora_api::decision - moved from caelumora_decision::client
pub enum Question { Choice { .. }, Noul { .. } }
pub enum Answer { Choice { .. }, Noul { .. } }
pub const MAX_OPTIONS: usize = 26;
pub const MAX_QUESTIONS: usize = 64;

/// The facet through which one decision model the platform offers is asked, built for whoever
/// asks it alone.
#[async_trait]
pub trait DecisionModelClient: Send + Sync + Debug {
    /// The model's name, as the configuration gives it.
    fn name(&self) -> &str;
    /// Whether what is classified `classification` may be sent to the model.
    fn may_receive(&self, initiator: ActorId, classification: &Classification) -> bool;
    /// The answers to `questions` about `state`, which is classified `classification`, by the
    /// name of each. `ModelError::Unfit`, nothing sent, for what it may not receive.
    async fn decide(
        &self,
        initiator: ActorId,
        classification: &Classification,
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Answer>>;
}

// caelumora_model
/// A decision model the platform offers: one entry of `plugins.model.decision`, served by the
/// backend its `type` names.
#[async_trait]
pub trait DecisionModelProvider: Send + Sync + Debug {
    /// What its server is cleared for.
    fn destination(&self) -> &Classification;
    /// The answers to `questions` about `state`, its credential resolved for `asking`.
    async fn decide(
        &self,
        asking: &SecurityContext,
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Answer>>;
}

impl ModelManager {
    /// The client of the decision model `name` - the one there is where `None` -, for whoever
    /// `acting` acts for alone.
    pub fn decision_client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<Arc<dyn DecisionModelClient>>;
}
```

- **`caelumora-model`** gets the section `plugins.model.decision`, by name, the kind
  `decision_model` (`name`), which provides the `DecisionModelClient` as `image_model` provides its
  facet, and the client that logs who asks. `ModelManager` becomes public for the one method
  above, for the policy, and holds the `ConfidentialityPolicy`.
- **`caelumora-model-systemone`**, new: `DecisionClient`, its fake-server tests and the backend
  `systemone` (`endpoint`, `model`, `keep_alive`, `destination`), registered as a plugin as
  `caelumora-model-comfyui` is. `DecisionError` goes: a call answers `ModelError`. Listed in
  `Cargo.toml`, `scripts/crates.sh` and by both programs.
- **`caelumora-decision`** keeps the policy alone, and depends on `caelumora-model`. The policy
  names its model by `name` and builds its client as `image_model` does; `endpoint`, `model`,
  `keep_alive` and `destination` leave its parameters. `threshold`, `max_responders` and how senders
  are marked stay.
- **`destination` moves to the model**, so that it is said once and by the operator.

What is not only structure, each named here for the review:

- `operator_only` is taken off `decision_policy`: nothing in its parameters says where anything goes
  any more, which was its reason.
- The policy asks in the context of the channel's owner, the `Acting` of what is built for a
  channel, not in that of the event routed. `may_publish` does not depend on the context; which
  credential is resolved changes, to the owner's from the sender's.
- A provider is handed who asks, where a language model's is not: a server that takes a credential
  needs it, which `caelumora-model-ollama` avoids by refusing `endpoint.auth`.

`examples/config.yaml` names the decision model, `team-session` names it by `name`.

Tests: as those of `image_model` for the kind; a client refuses what may not reach its destination;
the backend's against the fake server, moved; the policy's, with a scripted client. Documents:
CHANNELS.md, COMPONENTS.md, CONFIGURATION.md, LLM-MODELS.md, ARCHITECTURE.md, CRATES.md, PLUGINS.md.
Stored data: templates and channels with a `decision_policy` of the old parameters have to be
deleted.

## Done when

- A decision model is named in `plugins.model.decision` and served by the backend `systemone` of
  `caelumora-model-systemone`.
- `decision_policy` asks one through a `DecisionModelClient` and behaves as before, but for what is
  named above; what it may not send is not sent, and a test holds it.
- An agent's component can be given a decision model by the kind `decision_model`.
- The crate `caelumora-decision` holds the policy alone.

## History

- 2026-11-08: Opened at the maintainer's request, drawn out of the plan of issue
  [550](550-a-skill-is-an-entity-alone.md), whose step 2 this replaces and which waits for it.
  Decided with the maintainer: a crate of its own, `caelumora-model-systemone` (option B).
