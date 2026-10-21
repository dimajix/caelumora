# 443 — An error tells a person only an English text, which a frontend cannot show in their language

**Status:** open
**Type:** feature
**Priority:** low
**Area:** User interaction
**Summary:** What an error tells a person - a problem detail of the REST API, what a failed turn or command tells its owner - is an English text written by the runtime, with nothing beside it from which a frontend could say the same in the person's language.

## The problem

The REST API answers an error as problem details with a stable `code` (`name_taken`,
`invalid_value`, `skill_forbidden`, …) and a `detail`, an English sentence the runtime wrote: "the
summary must not consist of whitespace alone", the name that is taken, what was refused. The web
UI shows the `detail` (`caelumora-web-ui/src/errors.ts`). It could choose a text of its own by the
`code`, but what makes the text say something - which field, which name, which kind, which limit -
is only inside the English sentence. The same holds for what a person is told in a channel when a
turn, `/wakeup` or `/nudge` fails (`caelumora_runtime::error::user_message`).

Agents answer in the language of their channel ([56](56-response-language.md),
[425](425-session-language.md)); what the runtime itself tells the people in it does not.

## Considerations

*   **Not part of the prompt catalogue.** [57](57-prompt-catalogue.md) moves the texts given to
    models out of the code. Errors were deliberately left out there: they are error types whose
    `Display` also goes to logs, under the rule that an error message names what failed, and a
    frontend - not the runtime - knows the person's language.
*   **A code and its parameters.** What a frontend needs is what the `code` already starts: a
    stable identifier, and beside it the values the text is made of, by name (`field: "summary"`,
    `name: "Planner"`, `kind: "agent"`). Problem details allow extension members for that. The
    English `detail` stays, for clients that know no code and for the log.
*   **Where the parameters come from.** The variants of the error types already carry them as
    fields (`ValueError::Blank { field }`, `NameTaken`, `Refused`); the question is how they reach
    `ApiError` without each `From<…> for ApiError` building them by hand, and the same for a
    plugin's errors ([336](336-errors-of-a-plugin-shown-as-a-failure.md)).
*   **Notices in a channel.** What the moderator tells of its own (`Notice`, issue 418) is already a
    set of variants with fields, rendered as text; whether a frontend shows those by their variant
    too belongs to the same question.
*   **Errors told to models** (`ToolError`) are not part of this: a model reads the English text.

## Done when

*   An error the REST API answers carries, beside `code` and `detail`, the values its text is made
    of, by name, and [REST-SERVER.md](../docs/REST-SERVER.md#errors) describes them.
*   What a person is told in a channel when something fails can be shown by a frontend in their
    language.
*   The web UI uses it for at least the errors it shows most.

## History

- **2026-10-21** — Opened in the discussion of [57](57-prompt-catalogue.md): the maintainer
  decided that the texts of errors are not moved into the prompt catalogue, and that a frontend
  showing errors in the user's language needs technical information beside the text, as an issue
  of its own.
