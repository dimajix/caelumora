# 429 — How the messages of others are framed is configured three times

**Status:** rejected
**Area:** Code structure
**Summary:** `ollama_client`, `a2a_agent` and `decision_policy` each declare `sender_format`, `sender_style` and - the first two - `message_times` with their defaults in a configuration of their own and copy them into a `ModelProfile` by hand, so that a new setting of the framing is added in three places and missing in one goes unnoticed.

## The problem

How the messages of others are framed for a model - who said it (`sender_format`, `sender_style`,
issue 135), and when (`message_times`, issue 424) - is a rule of the `ModelProfile`
(`caelumora-model/src/context.rs`, `ModelProfile::frame`). Three components configure it, each in
a configuration of its own:

- `OllamaClientConfig` (`caelumora-ollama/src/client.rs`): all three, with
  `SenderFormat::configured_default` and `message_times_by_default`.
- `A2aAgentConfig` (`caelumora-a2a/src/agent/mod.rs`): the same three, the same defaults.
- `DecisionPolicyConfig` (`caelumora-decision/src/policy.rs`): `sender_format` and `sender_style`;
  no times, which whom to ask does not depend on.

Each copies the fields into its profile by hand and tests its defaults on its own. `message_times`
was added twice and left out once on purpose; a setting added later can be forgotten in one of
them without anything noticing, and the defaults can drift apart.

## Considerations

- **Where.** A small configuration type next to `ModelProfile` in `caelumora-model/src/context.rs`:
  the serde form of the framing fields with their defaults, which already live there
  (`SenderFormat::configured_default`, `message_times_by_default`), and the one conversion into the
  profile's fields. All three crates depend on the model; `caelumora-decision` does not depend on
  `caelumora-agents`. Not in the runtime's component helpers (`Params::read`), which know no
  models.
- **Embedded with `#[serde(flatten)]`**, so that the parameters in a configuration stay as they are.
  `ollama_client` keeps `reasoning_history` and `system_messages` beside it.
- **The decision policy gives no times.** Either it embeds the same type and refuses
  `message_times: true`, or it gets a type without times - which repeats the rest in small. To be
  decided.
- **Only the configuration.** What is stored, the `ModelProfile`, does not change.

## Done when

- The framing fields and their defaults are declared once, and each component reads them through
  that one type.
- The parameters of the three components are unchanged.

## History

- **2026-10-18** — Opened while issue 424 added `message_times` to two of the three configurations,
  after `SenderFormat::given` had become `ModelProfile::frame`.
- **2026-11-03** — Rejected by the maintainer: what is left of it is not worth what it costs. Since
  it was opened, `ollama_client` has become the kind `language_model`
  (`caelumora-model/src/language_model.rs`, issue 520) and `ModelProfile` has moved to
  `caelumora-domain/src/context/profile.rs`; besides the framing, `prompt_texts` is declared, copied
  into the profile and checked with `require_variant` in all three. The defaults are declared once
  already (`SenderFormat::configured_default`, `message_times_by_default`); only the attributes
  and the copying repeat. Embedded with `#[serde(flatten)]`, `deny_unknown_fields` still refuses
  an unknown parameter, but an error in an embedded field no longer names the parameter:
  `serde_path_to_error` sees only `.` (`invalid parameters: unknown variant 'header'` instead of
  `invalid parameter 'sender_format': …`), against the rule that an error of configuration names
  its parameter. A nested object (`framing: { … }`) keeps the names but changes the parameters of
  every configuration and of the stored personalities. The decision policy fits neither form: a
  `bool` that defaults to `true` cannot tell a `message_times: true` written from one left out, and
  giving it times would need the session's time zone, which `ResponderPolicy::choose` is not
  given. A type of `sender_format` and `sender_style` alone would leave `message_times` twice and
  `prompt_texts` three times - a new type for two fields.
