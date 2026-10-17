# 105 — The seed data exists twice, in the REST server and in the TUI

**Status:** done
**Area:** Maintenance
**Epic:** [74](74-refactoring-epic.md)
**Summary:** The demo data is seeded by both the REST server and the TUI, from two different definitions.

## The problem

`caelumora-rest-server/src/data.rs` (304 lines, 11 % of the crate) seeds a user, two personalities
and three session templates into the `EntityStore`. `caelumora-tui/src/main.rs:95-150` does the same
with its own `create_personality` and *different* prompts and components. The `--user` documentation
(`rest-server/src/main.rs:30-31`, "Owner of the templates … seeded") is wrong: everything is stored
with `SecurityContext::system()` and `security_attributes: Default::default()`; the name only
creates a `User` with an invented e-mail. The ten `ComponentDescriptor` literals (`data.rs:202-291`)
repeat seven identical fields each.

## Why it matters

Two demo data sets drift; a binary is not the place for data; and the fabricated `SecurityContext`
here is one of the three places issue [10](10-user-authentication.md) wants to see become one.

## Considerations

- One seed: a runtime module (`caelumora_runtime::seed`) or a seed file that `AppConfig` names,
  loaded by whichever binary starts first. Both binaries lose their copy. About a day including the
  TUI.
- If it stays for now: a `node(id, kind)` helper brings the descriptors to ~40 lines, and the
  `--user` doc says what the flag does.

## Done when

There is one definition of the demo data, outside both binaries.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 5.1.1).
- **2026-09-29** — Done, as agreed with the maintainer (option B, the data in `examples/`).
  `examples/seed.yaml` holds the two personalities and three session templates of the REST server,
  written as the store keeps them and with fixed ids; `AppConfig::seed` names seed files (none by
  default), and `examples/config.yaml` names this one. `caelumora_runtime::seed::load` puts into the
  store, as the system, what it does not have yet by id - idempotent without looking anything up by
  name, and a document changed in the store stays. Both binaries call it;
  `caelumora-rest-server/src/data.rs` and the TUI's own personalities and template are gone, and
  with the TUI's copy a bug: it created its personalities and template on every start, piling up
  copies in a JSON store. The TUI picks its template by name (`--template`, default `sassy-session`)
  and, without a configuration that seeds it, says to start with `--config examples/config.yaml`
  (the README does). One data set, the REST server's; on the way its personalities' `context` got
  the dependency on `create-plan-tool` it lacked, so its planning nudge now reaches the prompt.
  `--user` is gone: it created a user nobody used and was documented as the owner of data that
  belongs to the system. The YAML parsing of the configuration is shared (`config::parse_yaml`), so
  a seed quotes nothing of its input in errors and refuses unknown keys.
