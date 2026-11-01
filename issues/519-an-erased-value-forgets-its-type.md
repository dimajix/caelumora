# 519 — A value with its type forgotten is handed on apart from the key of its type

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `ErasedArc` forgets the type of the value it holds, so every call that hands one on takes the key of the type beside it, and where the two disagree the value is never found again and reads as a service nobody offered.

## The problem

Where a value is kept by its type - the services of the kernel, the facets of a component, what a
plugin binds to an agent's work - it is held as an `ErasedArc` (`caelumora-model/src/type_map.rs`),
an `Arc<T>` whose `T` is forgotten until it is asked for again (`get::<T>()`). The calls that hand
such a value on without knowing its type take the key of the type apart from the value, as issue
[516](516-the-runtime-trusts-the-agents-linked-into-it.md) named them:

- `TypeMap::insert_of(key, value)`;
- `LibraryBuilder::provide_of(key, service)` in `caelumora-api`, under the typed `provide::<T>`;
- the bindings of the readers and writers of an agent's work, kept by key in the kernel
  (`kernel/extension.rs`) and handed out by `ActorRuntime::reader_of` and `writer_of`.

Nothing checks that the key is the key of the value's type. A value inserted under the wrong key is
never found by `get::<T>()` of either type: it reads as a service nobody offered
(`ComponentError::NoService`), a face no plugin binds, a library's service missing - the wrong
error, far from the call that caused it.

## Why it matters

The typed calls (`provide::<T>`, `reader::<T>`, `TypeMap::insert`) make the key from the type and
cannot disagree; the erased ones exist for trait objects and for the kernel's side of the API, and
each new one repeats the pairing by hand. A value that knows its own type removes the pair, the
check and the class of error at once.

## Considerations

- **The value keeps its key.** `ErasedArc::of::<T>` records `TypeKey::of::<T>()`; `ErasedArc::key`
  answers it. The erased calls take the `ErasedArc` alone (`insert_of(value)`,
  `provide_of(service)`), and a binding is checked against the key it is kept under when it is
  made, not when it is asked for.
- **Where a key stays.** `reader_of(key)` asks for a type and answers a value: the key is the
  question there, and the answer is checked against it.
- **Cost.** A few signatures of the model, the API and the kernel; no behaviour changes but an
  error where a mismatch was silent.

## Done when

- No call hands on an `ErasedArc` together with a key it could disagree with; a value found by key
  is of the key's type, checked where it is kept.

## History

- **2026-11-01** — Opened from a finding of the review of step 6.1 of issue
  [516](516-the-runtime-trusts-the-agents-linked-into-it.md), where `TypeMap::insert_of` and
  `LibraryBuilder::provide_of` joined `reader_of` in taking a key beside the value.
