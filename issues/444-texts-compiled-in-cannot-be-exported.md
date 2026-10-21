# 444 — The texts compiled in cannot be had as files to begin a variant with

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Maintenance
**Summary:** A variant of the texts given to models is a directory of files of the form of those compiled in, but whoever runs the program without its sources has no copy of them to begin one with.

## The problem

Since [57](57-prompt-catalogue.md), a variant of the texts is a directory with a file per namespace
(`agents.yaml`, `core.yaml`, …), each of the form of the file compiled in, whose entries it
replaces one by one (`prompts` of the configuration, see
[CONFIGURATION.md](../docs/CONFIGURATION.md#the-texts-given-to-models)). The best way to begin one
is to copy the file compiled in and change it: it holds every key, its values and the comments on
what each text is for. But those files lie in the sources - `caelumora-model/src/prompts.yaml`,
`caelumora-agents/src/prompts.yaml` and one in each plugin - and an operator who runs a built
program has none of them, and cannot tell which keys there are or which values a text is given.

## Considerations

*   **A command of the program**: `caelumora prompts export <dir>` - beside `--config`, the first
    subcommand of `caelumora-app` - writing the file of every namespace the runtime has, as it was
    compiled in, into `<dir>`. The catalogue knows the namespaces and their files (added by
    `RuntimeBuilder::new` and the plugins), so it would keep their sources to give them back.
*   **As they were written, not as they are rendered**: the files, comments and all, not the
    texts flattened by key - the comments say which values a text is given.
*   **Of the default, or of a variant**: whether an export can write the texts of a configured
    variant merged over the defaults - to see what a model of it is given - is a further question.
*   **The TUI** has its own binary; whether it gets the command too is to be decided with it.

## Done when

*   The texts compiled in can be written as files by the program, one per namespace, of the form a
    variant takes.
*   [CONFIGURATION.md](../docs/CONFIGURATION.md#the-texts-given-to-models) says how.

## History

- **2026-10-21** — Opened when [57](57-prompt-catalogue.md) made variants directories of files of
  the form of those compiled in; the maintainer asked for an issue of its own for the export.
