# 571 — Approvals in the terminal take a UUID typed, and no rule for calls alike

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** An approval is answered in `caelumora-tui` only by typing `/approve` or `/deny` with its UUID, questions put to the user by typing their answers line by line, and nothing offers the rule "and from now on" for calls alike that the web UI offers on its approval card.

## The problem

*   **A UUID typed.** An approval requested is shown with its id, and answered by
    `/approve <uuid>` or `/deny <uuid>` (`caelumora-tui/src/main.rs`); questions by
    `/answer <uuid>`, then each answer on a line, or `/decline <uuid>`.
*   **No rule for calls alike.** The web UI offers, with its answer, a rule of the person derived
    from the call - the matcher the call's tool suggests (`ApprovalRequest::suggested`) -, which
    the user's controller adds (`UserController::add_approval_rule`) and their actor applies to
    later calls ([APPROVALS.md](../docs/APPROVALS.md#rules-of-a-person)). The terminal offers none.

The kernel offers both already; the terminal does not use them.

## Considerations

Decided (2026-11-14), on the ways of drawing of issue
[569](569-the-terminal-mixes-its-output-into-the-line-typed.md):

*   **What is asked of the user** - an approval pending in the channel talked in, questions put to
    them - is a model the ways of drawing share, with its choices: for an approval "yes", "yes, and
    from now on for `<the matcher suggested>`" - where one is suggested, with its reach as the web
    UI offers it -, and "no"; for questions, their choices or an answer of one's own.
*   **`full`** shows it as a choice above the line typed, as Claude Code does: arrows and Enter, or
    the number of a choice; Esc puts it aside to type on, and it stays shown as waiting.
*   **`line`** shows the choices numbered, and takes the number typed.
*   **The commands stay** as a way without a choice, named by the rules of issue
    [570](570-the-terminal-s-commands-and-its-sessions.md): without an argument for what is shown,
    else by its number or the beginning of its id - no UUID typed in full.
*   The person answers the call first; the rule is added after, as on the web UI.

## Done when

- An approval and questions are answered without typing an id, in both ways of drawing.
- An approval can be answered with a rule for calls alike, where its tool suggests one.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal, with the choice in both ways of
drawing decided.
