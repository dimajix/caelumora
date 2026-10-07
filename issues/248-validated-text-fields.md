# 248 — Text fields are checked by whoever writes them, not by their type

**Status:** in progress
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
- **2026-10-07** — Decided with the maintainer:
  - **One module for values with rules**, `caelumora_model::value`, one file per subject: `name`
    (`Name`, `ScopedName`), `language` (`Language`), `text` (`Text<R>` and its rules). Not `types`,
    which would say nothing about what is in it. What they share lives in `value/mod.rs`.
  - **One API for all of them**: a trait with the field's name (`FIELD`), `new(&str)`, `as_str()`
    and `optional(Option<&str>)` - a blank text is `None`; one error type, `ValueError`, whose
    every variant names the field; a macro for the trait implementations that are alike
    (`TryFrom<String>`, `From<…> for String`, `AsRef<str>`, `Display`, `Debug`, serde: written as
    a string, checked when read). `Language::parse` becomes `new`; `NameError`, `LanguageError` and
    `TextError` are folded into `ValueError`. No `Deref<Target = str>`: for a `Name`, `&*name ==
    text` would compare exactly where `name == text` compares folded.
  - **`Text<R>`** with the rule as a marker type (`TextRule`: `FIELD`, `MAX_CHARS`, `ONE_LINE`),
    the rules kept together in `value/text.rs`, since several owners share them: `Summary` (one
    line, 300), `Briefing` (16 000), `Introduction` (1000; a user's and a personality's),
    `Description` (one line, 300; the catalogue line of a personality and a session template).
  - **Whatever could be looked up by its name is a `Name`** - a session's, too. A user's name stays
    a `String`: a user provider decides it.
  - **Messages, notes, plan steps, tool and command descriptions are left out**: models and code
    write them, not people into a field with a rule; failing to read a model's answer would end
    its turn, and notes are in a digested ledger.
  - **Stored documents that do not fit fail to load**; old data is not migrated.
  - Done in three steps: the module and the shared API for `Name` and `Language`, without a change
    in behaviour; `Text<R>` for the fields checked today; then the fields not checked so far.
- **2026-10-07** — Step 1 done: `caelumora_model::value` holds `Name` and `Language`
  (`value::name`, `value::language`), with `ValueError` and `impl_value!` in `value/mod.rs`.
  `Language::parse` is `Language::new`; `NameError` and `LanguageError` are gone, and their messages
  now name the field ("the name must not be empty"). No behaviour changed otherwise.
- **2026-10-07** — Step 2 done: `value::text` holds `Text<R>` with the rules `Summary`,
  `Briefing` and `Introduction`; the summary and briefing of sessions, channels and their
  templates, a user's introduction, their events and snapshots are of these types, checked when
  made and when read. `TextRule` is a trait now; `Session::described`, the controllers'
  `checked_description`, `TemplateError::Description`, `UserProfile::introduce`/`introducing` and
  `MAX_INTRODUCTION_CHARS` (now `Introduction::MAX_CHARS`) are gone. The REST server makes the
  values from the strings of a request (`DescriptionRequest::description`,
  `ProfileRequest::introduction`) and still answers `400 invalid_description` or
  `invalid_introduction`. A member's introduction in a snapshot stays a `String` until step 3,
  when a personality's is an `Introduction` too.
- **2026-10-07** — A text is checked, not changed (maintainer's decision): `Text::new` keeps it as
  written instead of trimming it; a blank one is still `Empty`, and `optional` still makes a blank
  one `None`. Trimming is the input's - the web UI trims what it sends. `Name` and `Language` stay
  tidied, since they identify something (`value/mod.rs`).
- **2026-10-07** — Only an empty text is none (maintainer's decision): `optional` takes a missing or
  empty text for `None`, and anything else for a value or an error - a text of whitespace is not
  taken for nothing, since that would not be transparent. `Text::new` refuses an empty text
  (`ValueError::Empty`) and one of whitespace alone (`ValueError::Blank`), and keeps any other
  exactly as written. A `Name` or `Language` of whitespace is an error, too, being tidied first.
