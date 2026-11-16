# 589 — Loose ends of how frontends show a tool call and its result

**Status:** in progress
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** Since issue 586 a tool result carries its own presentation, but the web UI and the terminal's `Followed` still go by the call's alone, TOOLS.md leaves `withheld` out of the statuses a result tells, and CODING-GUIDELINES.md can be read two ways on whether a function may import for itself.

## The problem

Issue [586](586-the-terminal-shows-a-tool-call-by-its-raw-arguments.md) left four findings that
were not its to decide. Each is small; they are kept together because they came from the same
work, and each can be done, or rejected, on its own.

1.  **The web UI reads the call's presentation alone.** A `ToolCallResult` carries the
    presentation of its call (`ToolCallResult::presentation`, stamped by `ToolCallResult::of`),
    so that a tool may one day choose how its result is shown. TOOLS.md states the rule: a call
    is shown by its own presentation until its result comes, and then by the result's. The
    terminal's full-screen view keeps it (`screen/full/call.rs`); the web UI does not: it picks
    the handler by `call.presentation` (`ToolCallView.tsx:1099`), and finds the commands that
    ended by it (`endedCommands`, `messages.ts:130`). Today both are always equal, so nothing is
    shown wrong; the day a tool gives its result another presentation, the web UI shows it by the
    call's.
2.  **`Followed` tells a quiet result by finding its call.** Whether a successful result is left
    out as `quiet` is decided by finding its call among `Followed::quiet_calls`
    (`caelumora-tui/src/followed.rs:677`). Where the call is not there, the result is printed
    alone - "attach answered: …" - though its call was never shown:
    *   the channel is opened, continued or switched to, and of its history the last
        `HISTORY_ITEMS` are printed: a quiet call just before them, with its result among them;
    *   the agent's turn ends, which forgets its quiet calls (`quiet_calls.retain`), and a result
        of one of them comes after.

    Both need a quiet call to stand at an edge; rare, and what is printed is a line too many, not
    a line lost.
3.  **TOOLS.md leaves out `withheld`.** Under "Results" the list of statuses a result tells
    names every `ToolResultStatus` but `withheld`, which is described elsewhere in the document
    (a result that may not flow into the channel). A reader of the list takes it for complete.
4.  **Imports inside a function.** CODING-GUIDELINES.md says "Import what you use with `use` at
    the top of the module" among its rules for every change, and under "Imports" that
    `scripts/import-blocks.sh` checks the one block "of a function's imports too" - which
    presumes a function may import for itself. The code does both: some 255 imports stand inside
    functions, most in tests (`caelumora-agents/src/turn/ending.rs:174` and `:295`, many in
    `caelumora-kernel/src/view/mod.rs`). A reviewer of issue 586 asked for them to move, another
    let them stand.

## Why it matters

The first two are the same rule followed in one place and not in another: the result says how it
is shown, and a frontend that asks the call instead is right only while the two agree. The third
is a document that a reader trusts to be complete. The fourth costs every review a discussion of
where an import may stand.

## Considerations

**1. The web UI.** The handler is chosen by `result?.presentation ?? call.presentation`: the
result's once it came, the call's before - the `shell` handler shows a command while it runs, by
the call. `endedCommands` reads results only, and takes the result's. Nothing changes what is
shown while the two agree. Alternatively it waits for the first tool that chooses its result's
presentation; that tool's issue would then have to remember the web UI.

**2. `Followed`.** A successful result is left out by its own presentation, `result.presentation
== ToolPresentation::QUIET && result.status.is_ok()`, whether its call was found or not.
`quiet_calls` is then needed only to print the call of a quiet result that failed, as the web UI
shows it; where that call is not found either, the result is printed alone, as today. A result
stored before issue 586 carries no presentation and reads as `default`: of an old history, a quiet
result whose call lies outside the page is printed as today. Documenting the edge and holding it
by a test is the other option; the change is a line, the documentation would be longer.

**3. TOOLS.md.** `withheld` joins the list, with what its message says: the classification the
result may not flow into the channel by, and not to obtain it another way.

**4. Imports inside a function.** Two ways, both checked by a tool afterwards:

*   **Only at the top of the module**, also in tests. The rule reads as it is written; the 255
    move, by a script per file, and a lint or `scripts/import-blocks.sh` refuses new ones. A test
    module's block grows by what single tests use, and its imports no longer show which test needs
    what.
*   **Allowed inside a function, in one block at its start**, as `import-blocks.sh` already checks.
    The rule for every change says so; nothing moves. What a single test or a function used once
    needs stays with it.

The second matches the code as it is and what the check already does; the first gives one place
to look. The maintainer decides; the decision is written into CODING-GUIDELINES.md.

## Done when

- The web UI chooses a call's handler by its result's presentation once the result came, and its
  call's before, and finds the commands that ended by the result's.
- `Followed` leaves out a successful result by its own presentation; a test holds a quiet result
  whose call was not printed.
- TOOLS.md lists `withheld` among the statuses under "Results".
- CODING-GUIDELINES.md says whether a function may import for itself, the code keeps it, and a
  tool checks it.

## History

2026-11-16: Opened at the maintainer's request from the findings of issue 586 left open: the web
UI and `Followed` go by the call's presentation, TOOLS.md leaves out `withheld`, and the rule on
imports inside a function reads two ways.

2026-11-16: Planned with the maintainer, a step per finding: TOOLS.md lists `withheld`; `Followed`
leaves out a successful result by its own presentation; the web UI shows a call by
`presentationOf(call, result)` - the result's once it came, a result without one as `default` -,
and `commandState` reads a result of `shell` alone; a function may import for itself, in one block
at its start, which CODING-GUIDELINES.md says and a check holds (option B). Results stored before
issue 586 carry no presentation and are shown as `default`; no code reads them otherwise.

2026-11-16: Step 1 done: TOOLS.md lists `withheld` under "Results", pointing to where a classified
result is described, not repeating it.

2026-11-16: Step 2 done: `Followed` leaves out a result presented `quiet` that succeeded by its
own presentation; `quiet_calls` serves to print the call of one that did not.

2026-11-16: Step 3 done: the web UI shows a call by `presentationOf(call, result)`, in
`ToolCallView` and `withoutQuietCalls`, and `commandState` reads a result presented `shell` alone,
as `command_state` does - `endedCommands` reads the results, not the calls.

2026-11-16: Step 4 done, option B: CODING-GUIDELINES.md says that what only one function or block
uses - a closure, an `async` block - may be imported at its start, in one block, and
`scripts/import-blocks.sh` reports an import after code, in a module, a function or a block: it
tells levels of indentation, not functions, so a block is held as a function is. It found nine
places, which now import before their code: eight functions, most of them tests, imported after
their first statements, and `caelumora-store-home` declared a constant before its imports.
