# 500 — A copy never stored carries the revision of one stored once

**Status:** done
**Type:** question
**Priority:** low
**Area:** Persistence
**Summary:** A new entity carries the first revision and `create` stores it at that revision, so that a copy never stored cannot be told from one stored once, and its `update` passes against a document created meanwhile under its id.

## The problem

Every entity carries its revision (`Stored::revision`, issue
[492](492-a-document-is-written-under-two-locks.md)); a new one carries the first,
`Revision::default()`, and `EntityStore::create` stores it at that revision. A writer that holds a
copy it never stored - one it made to create when there is none - and that is created meanwhile by
another, in another instance, holds a copy at the same revision as the one stored: an `update` of
it passes, and replaces what the other created.

The one writer that did so, the `UserController` with a profile its user never wrote, was changed
in 492: it holds no profile until one is stored, creates rather than changes, and takes a creation
meanwhile as a refusal. Any writer that creates or updates a copy of its own repeats the gap.

## Why it matters

What is lost is a whole document, silently. The rule that avoids it - create, never update, a copy
never stored - is written nowhere but in the one controller that keeps it.

## Considerations

- **`create` writes the revision after the first.** A copy never stored is then at a revision no
  stored document has. `create` would have to give the caller the revision written - `&mut T`, as
  `update` - or every creator read the document again before it changes it.
- **A revision that says "never stored".** `Option<Revision>` or a variant of it: told apart in
  the type, at the price of a case every storage handles.
- **Leave it, and write the rule down.** STORAGE.md says that a copy never stored is created,
  never updated.

## Done when

- Decided, and either built or written down as a rule.

## History

- **2026-10-29** — Opened from the review of the second step of issue
  [492](492-a-document-is-written-under-two-locks.md), which closed the gap for the profile
  controller and recorded the question.
- **2026-10-29** — Decided for the first option, in two steps: `create` writes the entity as the
  revision after the one it carries, as `update` does, and leaves the revision written in the
  value; a copy never stored then carries the first revision, which no stored document has, and
  its `update` is refused as outdated. Step 1 done, without a change of behaviour:
  `EntityStore::create` and `seed::create_unless_taken` take `&mut T`, and every writer that
  creates goes on with the value it gave the store - a manager announcing and returning it,
  `SessionManager::persist` its session, its own project and its channels (`establish` takes
  the project as it takes the session), the seeds a copy of each of their documents.
- **2026-10-29** — Step 2 done: `CheckedStore::create` writes the entity as the revision after the
  one it carries and leaves it in the value, as `update` does - both through one step that counts
  the revision up and takes it back if the write fails, which also takes it back when the value
  cannot be serialized. The store suite holds every backend to it: what is created is at the
  revision after the first, a creation refused leaves the value as it was, and a copy never
  stored is refused as outdated in place of one created meanwhile. The user's profile controller
  holds an empty profile again while none is stored, changes the profile it holds, and creates it
  where there is none, without telling the two apart itself. STORAGE.md states the rule.
  Documents stored before at the first revision keep it until they are written again.
