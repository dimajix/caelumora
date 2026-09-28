# 105 — The seed data exists twice, in the REST server and in the TUI

**Status:** open
**Area:** Maintenance

## The problem

`caelumora-rest-server/src/data.rs` (304 lines, 11 % of the crate) seeds a
user, two personalities and three session templates into the `EntityStore`.
`caelumora-tui/src/main.rs:95-150` does the same with its own
`create_personality` and *different* prompts and components. The `--user`
documentation (`rest-server/src/main.rs:30-31`, "Owner of the templates …
seeded") is wrong: everything is stored with `SecurityContext::system()`
and `security_attributes: Default::default()`; the name only creates a
`User` with an invented e-mail. The ten `ComponentDescriptor` literals
(`data.rs:202-291`) repeat seven identical fields each.

## Why it matters

Two demo data sets drift; a binary is not the place for data; and the
fabricated `SecurityContext` here is one of the three places issue
[10](10-user-authentication.md) wants to see become one.

## Considerations

- One seed: a runtime module (`caelumora_runtime::seed`) or a seed file that
  `AppConfig` names, loaded by whichever binary starts first. Both binaries
  lose their copy. About a day including the TUI.
- If it stays for now: a `node(id, kind)` helper brings the descriptors to
  ~40 lines, and the `--user` doc says what the flag does.

## Done when

There is one definition of the demo data, outside both binaries.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 5.1.1).
