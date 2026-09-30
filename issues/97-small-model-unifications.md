# 97 — Six default functions for `Confidentiality::Public`

**Status:** done
**Area:** Maintenance

## The problem

The same constant has six helper functions: `default_confidentiality` in
`channel.rs:235` and again in `template.rs:84`, `default_moderator`
(`template.rs:80`), `public_reasoning` (`agent.rs:122`), `public`/`is_public`
(`message.rs:98/102`), `ToolConfidentiality::is_public` (`tool.rs:101`).
Channels default to `Confidentiality::CHANNEL_DEFAULT`, message parts to
`Public`; each site spells its own function for one of the two.

## Why it matters

Whoever adds the next labelled field writes a seventh.

## Considerations

`impl Default for Confidentiality` (= `Public`) plus
`Confidentiality::is_public(&self)`; for channels one `fn channel_default()`
in `confidentiality.rs` that `channel.rs` and `template.rs` share. Six
functions become two.

## Done when

No module outside `confidentiality.rs` defines a default for a
confidentiality level.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.10). Originally a collection of
  small model duplications; split the same day at the maintainer's request
  into this issue and [112](112-id-types.md)–[117](117-message-prev-id.md).
- **2026-09-30** — Resolved by step 1 of
  [133](133-classification-model.md), which replaced the levels on channels
  by `Classification` and the labels on message parts by `Visibility`. Both
  define their default in `confidentiality.rs` (`impl Default`, with
  `Classification::CHANNEL_DEFAULT` for channels); the fields use
  `#[serde(default)]`, and `default_confidentiality` (twice),
  `public_reasoning` and `public`/`is_public` in `message.rs` are gone.
  `ToolConfidentiality::is_public` remains as the test for the default
  flags of a call; the type is renamed in step 3 of 133.
