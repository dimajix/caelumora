# 445 — A personality that changes one parameter of its model repeats all of them

**Status:** done
**Type:** feature
**Area:** Code structure
**Summary:** A personality that needs one parameter of its model changed - the variant of the texts it is given, say - has to write out a model of its own with every parameter of the configured default, which then no longer follows a change of the default.

## The problem

The model of an agent is a facet of which it has exactly one (`LLMClient`, with its
`ModelProfileProvider`, see
[COMPONENTS.md](../docs/COMPONENTS.md#facets-of-which-there-is-exactly-one)). Where a personality
names none, it is given the one `AppConfig::component_defaults` configures - in the example
configuration `ollama_client` with `model`, `sender_format`, `sender_style` and `context_size`.
Where it names one, that one replaces the default whole: nothing of the default's parameters is
taken over.

So a personality that is to differ in one parameter writes out all of them. Sassy in the example
seed is given the variant `gemma4` of the texts ([57](57-prompt-catalogue.md), `prompt_texts`) and
for that repeats the default's four parameters beside the one it changes. Every other parameter a
deployment sets on its default - the endpoint, `think`, `reasoning_history`, `system_messages`,
`message_times` - would have to be repeated too.

## Why it matters

- **The copy drifts.** A change of the default - another model, a larger `context_size`, a new
  endpoint - reaches every agent but those that changed one parameter of it, and nothing tells.
  The seed and the configuration are written by different people: the seed cannot know what the
  configuration of a deployment says.
- **Which parameter was meant to differ** cannot be told from the copy: the one changed and the
  ones repeated look alike.
- **It works against 57.** A variant of the texts is chosen per model, and the natural place to
  choose it for one personality is its model; that is exactly where the repetition is.

## Considerations

- **A component that names its base.** A descriptor names the default it builds on - by the id of
  an entry of `component_defaults` (`base: model`) - and gives only the parameters that differ; its
  kind is the base's, its parameters the base's with its own merged over them. Explicit: what
  differs is what is written. Open: whether the merge is shallow or deep (an `endpoint` with only
  its `url` changed), whether a base may also be a preconfigured component (`kind: ref`), and what
  a base that is not configured does to the agent - kept from being built, as a variant that is not
  configured is.
- **The same id overrides.** A personality's component with the id of a configured default, and
  of its kind, has its parameters merged over the default's. Nothing new to write, but implicit: a
  component that happens to share the id changes meaning, and it is the override
  [270](270-personality-inheritance.md) decided against for the components of a bundle ("a
  component of a bundle that the personality writes again is a configuration error ... not an
  override"). A second rule for defaults beside that one would need a reason of its own.
- **The variant chosen by the personality.** A field of the personality - or of its agent -
  naming the variant of the texts, given precedence over the profile's. Solves Sassy's case only:
  57 made the variant a property of the model's profile on purpose, since the texts are written for
  a model; and every other parameter keeps the problem.
- **Leave it.** Document that a model of its own is a whole model, and accept the copy. Cheap, and
  honest while few personalities differ; the drift stays.
- Whatever is chosen is a rule of how a `ComponentContainer` takes its defaults
  (`ComponentRegistry::default_for`, `ComponentContainer::with_defaults`), not of a kind: it holds
  for every facet of which there is exactly one - the `ContextProvider`, the compaction - not only
  for the model.

## Decision

A component that names its base, written as a `ref` is: a descriptor of the reserved kind `default`
(`COMPONENT_DEFAULT_KIND`), whose `params` name the default it builds on by its `id` and give the
params that differ - `params: { id: model, params: { prompt_texts: gemma4 } }`. It is resolved where
a `ref` is (`ComponentRegistry::resolve`), before anything is built, to a descriptor of the
default's kind with the default's params and its own over them; its id, tags and fail policy are
its own.

1.  **The merge is one level deep.** Each param it gives replaces the default's of its name whole.
    Merged deeper, a personality could change `endpoint.url` and keep the default's
    `endpoint.auth`, and so have a secret the operator configured for their server sent where the
    personality points it - not for `ollama_client`, which refuses an `auth`, but for any single
    whose kind takes one. Lists would need rules of their own besides.
2.  **The base is found by its id**, among the defaults the container was built with first
    (`ComponentContainer::with_defaults`), then among the configuration's. The ids of
    `component_defaults` become unique, and names a seed may refer to; a base that is not
    configured keeps the component from being built, named by the error.
3.  **Not the same id overriding**, which is the override 270 decided against; **not a variant
    chosen by the personality**, which solves the texts only and works against 57; **not a `ref` as
    a base**, since a `ref` is a list since 270, and overriding an entry of it a second mechanism. A
    component of `kind: default` may stand among preconfigured components, building on the defaults
    of whichever container includes them; a default cannot build on a default.
4.  **What an operator configures stays the operator's.** A component that builds on a default of
    an `operator_only` kind counts as written (`ComponentRegistry::require_acceptable`): what the
    operator decides are the params.
5.  The form of the descriptor stays as it is - no `base:` field beside an optional `kind`, which
    would be a descriptor without a kind as a state of its own -, at the price of `params` nested
    in `params`.

## Done when

- A personality can change a parameter of its model without repeating the others, and follows
  every other change of the configured default - or the issue is rejected with the reason why the
  copy is the better rule.
- [COMPONENTS.md](../docs/COMPONENTS.md) and [CONFIGURATION.md](../docs/CONFIGURATION.md) say how,
  and Sassy in the example seed names only what differs.

## History

- **2026-10-21** — Opened when Sassy in the example seed was given the variant `gemma4` of the
  texts and for that had to repeat every parameter of the configured model; the maintainer asked
  for an issue.
- **2026-10-21** — Decided (see *Decision*): the reserved kind `default`, analogous to `ref`, with
  the params merged one level deep; work started on the branch `issue-445-component-base`.
- **2026-10-21** — Implemented on the branch: `ComponentRegistry::resolve` builds a `kind: default`
  on the default it names - written in a configuration or among preconfigured components -,
  `kind_of` tells the default's kind, and `set_defaults` refuses two defaults of one id and a
  default that builds on one. Sassy in the example seed names only `prompt_texts`; the app's test
  runtime is given the example configuration's defaults. [COMPONENTS.md](../docs/COMPONENTS.md)
  and [CONFIGURATION.md](../docs/CONFIGURATION.md) describe it. The stored `sassy-assistant` has
  to be deleted for the seed to take effect.
- **2026-10-21** — Done: accepted by the maintainer and merged into `main`.
