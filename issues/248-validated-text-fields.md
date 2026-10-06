# 248 — Text fields are checked by whoever writes them, not by their type

**Status:** open
**Area:** Code structure

## The problem

Free text in the model is a `String`, and whether it fits its field is checked where it is written:
`UserProfile::introduce` checks an introduction, `Session::described` a session's or channel's
summary and briefing (issue [245](245-session-and-channel-descriptions.md)), a template's when it is
used. The rules - `TextRule`, kept by the field's owner as a constant - are in one place, but
nothing makes anyone apply them: a `Session` built or deserialized with a summary of ten thousand
lines is a valid `Session`, and a new writer of the field has to remember the check.

Other fields are checked by their type: `Name` (issue 177) and `Language` (issue 56) are newtypes
that can only be made through their constructor and are checked when they are read
(`#[serde(try_from = "String")]`). Each was written by hand, with its own error type, and each says
by its name what it means.

And some fields are not checked at all, though people write them and they reach prompts or lists:

- `Session::name` is a `String` of any length - deliberately not a `Name`, since sessions are not
  looked up by name (`name.rs`), but nothing bounds it either.
- `Personality::introduction` goes into the system prompt of every agent in a channel, like a
  user's introduction, but is not checked like one; `Personality::description` and
  `SessionTemplate::description`, the catalogue lines, are not checked at all.
- A message's content, a note's text, a plan's steps are written by people or models and are bounded
  by nothing in the model.

## Why it matters

An invariant held by every writer is held until one forgets. A field whose type says what it may
hold cannot hold anything else - and is checked once, wherever it comes from: a REST request, a
template written by hand, a store document. Python's Pydantic is the model the maintainer has in
mind: a field declares its constraints, and assigning to it validates.

## Considerations

- **What Rust offers.**
  - `nutype`: a derive macro for newtypes with `sanitize(trim)`, `validate(len_char_max = …,
    predicate = …, regex = …)` and serde that validates on deserialization. Closest to Pydantic per
    field; but a generated error type per field whose messages do not name the field, and a
    dependency.
  - `garde`, `validator`, `serde_valid`: derive validation of whole structs, called after they are
    built (`.validate()`). An invalid value stays representable - less than a newtype gives.
  - **Our own generic type**: `Text<R: TextRule>`, a `String` with a marker type whose associated
    constants are the rule - `FIELD: &'static str`, `MAX_CHARS`, `ONE_LINE` - and `pub type Summary
    = Text<SummaryRule>`. Associated constants may be strings, unlike const generics, so the message
    names the field; one error type (`TextError`); `Deref<Target = str>`; deserialization checks.
    No dependency. Recommended when this was first discussed (in issue 245).
- **What it covers.** Free text with a length, one line or many, no control characters, trimmed -
  and, where a field needs it, a pattern (a regex: an e-mail address, an identifier). Whether `Name`
  becomes one of them is a question of its own: it also tidies whitespace and is compared by a
  folded key, which is more than a rule of text; it could be built on `Text` or stay beside it.
- **Where the check happens.** On deserialization as well as on construction: a template is checked
  when the seed is loaded, not when a session is made from it; a stored document that does not fit
  fails to load. Since every writer has checked so far, nothing stored should fail - but a stored
  personality's introduction was never checked and might.
- **REST requests stay strings.** A request body that fails to deserialize is answered by axum's own
  rejection, not by `ApiError` (`400 invalid_description` and the like); requests keep `String`
  and are converted in the handler.
- **Blank is none.** Made from a text, a blank one is `None`; read from a document, an empty string
  is an error, since only `None` is stored.

## Done when

The text fields people or models write are of types that check them, made in one way, with one
error type naming the field; the checks done by their writers today are gone; and the fields that
are not checked today are, or say why not.

## History

- **2026-10-06** — Opened while issue [245](245-session-and-channel-descriptions.md) was
  implemented, which introduced `TextRule` - the rule kept by the field's owner, applied by its
  writers. The maintainer asked whether the fields could be types that check themselves, as with
  Pydantic; deferred to be done thoroughly, across the model.
