# 571 — Approvals in the terminal take a UUID typed, and no rule for calls alike

**Status:** done
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

**2026-11-14** - Decided with the maintainer before the work began, in three steps: a restructuring
without a change of behaviour, the talk's state - its channels, the questions being answered, the
sessions - one type, `Talk`, whose methods take a line typed in; then what is asked of the user as
one model, `Asking` (the options of the program becoming `Invocation`), asked of its own accord
as an approval waits or questions are put to the user, Esc putting it aside; then the rule. An
approval offers "yes", "yes, and from now on for the calls the tool suggests" in this channel or in
this session, without an end, and "no" - no rule that denies, none everywhere, no time limit, no
matcher edited: what more there is, the web UI offers.

**2026-11-14** - Step 1 done, without a change of behaviour: the talk is `Talk` (`talk.rs`) - its
channels, the questions being answered, the sessions of the directory -, whose methods take in a
line typed and what the channels show; `commands.rs` holds the terminal's commands alone
(`TerminalCommand`, `terminal_help`). What the program is asked by its options is `Invocation`.

**2026-11-14** - Step 2 done: what is asked of the user is `Asking` (`asking.rs`), which `Talk`
asks of its own accord - the oldest approval pending in the channel talked in, else the oldest
questions put to the user there, none answered already -, and each way of drawing shows its own
way (`Screen::ask`): `full` as a choice above the line typed (`screen/full/choosing.rs`), Esc
putting it aside and taking it up again, `line` with the choices numbered. The commands name an
approval or questions by the beginning of the id (`commands::beginning_with`, which the sessions
use too), without one the one asked - not by a number, which names a choice. While questions are
asked, a command - the terminal's or the channel's - is carried out, and only a message answers;
a message typed while they are put aside declines them, as the user's actor does. The tests build
an approval request by `approval::testing::make_approval_request` of the domain.

**2026-11-14** - Decided with the maintainer after the review of step 2, and done in it: an
approval is asked only where the user may approve its agent's calls, as the web UI offers its
buttons (`Followed::may_approve`); what is asked is held as shown or put aside (`Asked`), not by a
flag beside it; switching the channel asks anew of the channel switched to. Step 3 begins with the
choices told by their meaning instead of by their index. That the terminal remembers what the user
answered (`Talk::answered`) beside the owners of approvals and questions is an issue of its own.

**2026-11-14** - Step 3 done: what a choice means is `Offered` (`asking.rs`), which `Asking`
answers its choices by - the call approved, approved with the calls alike from now on in its
channel or its session, or denied; a question's choice -, and which `screen/words.rs` words for both
ways of drawing. An approval whose tool suggests a matcher, and which no rule of a place asked a
person to decide, offers the calls alike; chosen, the call is approved first, then the user's rule
added by their controller (`UserController::add_approval_rule`), and where that fails, the answer
stands and the user is told so.
