# Issues

Every issue is a file `issues/<number>-<slug>.md`. Its field lines at the top say what it is, where
it belongs and how it stands; the reasoning, the discussion and the decision follow below them -
there it may be as long as it needs to be. Two overviews are generated from the field lines:

- **[INDEX.md](INDEX.md)** - every issue, one line each, under its area and by number.
- **[OPEN.md](OPEN.md)** - the issues that are `open` or `in progress`: the bugs first, then by
  area, those of priority `low` last; within a section by priority, then by number.

Why the split: an overview is read every time anyone looks; an issue file only when someone works
on that issue. Keeping both in one file meant either the overview drowned in the detail, or the
detail was never written down. But the detail is the valuable part: why something is open, what
already argues against it, and what came out of the last attempt.

## The fields

Every issue file begins with its heading, `# <number> — <title>`, an empty line and its field
lines, each on a line of its own:

```markdown
# 381 — A channel nobody speaks in stays in memory with its actors

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Epic:** [378](378-resource-management-epic.md)
**Summary:** A channel nobody has spoken in for days keeps its loop, its actors and their states in memory until its session is stopped, and nothing notices that it is idle.
```

- **Status:** `open` · `in progress` · `done` · `rejected`. `rejected` means: looked at and
  deliberately not done, with the reason attached — because that reason is otherwise worked out
  again from scratch six months later.
- **Type:** `feature` · `bug` · `refactoring` · `epic` · `question`. A `bug` is behaviour that
  is wrong, a `refactoring` changes structure without changing behaviour, an `epic` brackets other
  issues and their order, a `question` is to be decided before anything is built. Required while
  an issue is `open` or `in progress`.
- **Priority:** `high` · `medium` · `low`. Required while an issue is `open` or `in progress`.
- **Area:** the subject the issue belongs to, one of the sections of INDEX.md: Observability and
  operations · Persistence · Sessions and channels · Identity and access management · Audit ·
  Tools and approvals · User interaction · Context and knowledge · Maintenance · Code structure
  · Integrations. The list and its order are kept in `issues/generate.sh`; a new area is added
  there.
- **Epic:** optional, a link to the epic the issue is part of; it has to be an issue of type
  `epic`. OPEN.md names it after the summary.
- **Summary:** the line of the issue in the overviews - one sentence, saying what is wrong, not how
  it will be fixed. Everything longer belongs below the fields. It stays on one line, however long,
  and contains no `|`.

## How this is maintained

- **The issue file is the one place** an issue's status, type, priority and area are kept.
  INDEX.md and OPEN.md are written from the files by `issues/generate.sh` and never edited by
  hand. The pre-commit hook (`.githooks/pre-commit`) runs it whenever an issue is part of a commit,
  on the issue files as they are staged, and adds the result to the commit; it is enabled once per
  clone with `git config core.hooksPath .githooks`. Without the hook, run the script after changing
  an issue; `issues/generate.sh --check` fails if the overviews are not up to date. The script
  refuses an issue whose fields are missing or unknown, naming the file and the field.
- **No issue ever disappears**, and **numbers are never reused** and never reassigned. A new issue
  gets the next free number, even if issues before it are closed. Numbers are referred to — from
  commits, from code comments, from the other documents; a number that moves eventually points at
  something else. Numbers are written with **two digits** (`01`, not `1`) so the file listing
  sorts correctly; from `100` onwards, three.
- **When an issue is resolved, the status changes.** Nothing else: no deleting, no rewriting of the
  description. What was once open and how it turned out is exactly the information a deleted issue
  no longer gives.
- **What happened goes under "History"** at the end of the issue file, with a date, as a new
  paragraph. The existing text stays, even once it is outdated — it is the record of the problem,
  not of the solution.

## Creating a new issue

A new issue is opened on `main` alone, from its newest version, in a commit of its own that is
pushed at once; its branch comes only with its implementation. Why, and how exactly, is in
[AGENTS.md](../AGENTS.md#branches-and-commits): a number is taken where everybody takes them, and is
published before anyone else can take it.

1. Check [INDEX.md](INDEX.md) first — an issue for it may already exist, in which case add to that
   file's `## History` rather than opening a second one.
2. Take the next free number and create `issues/<number>-<slug>.md` with the heading, the fields,
   the problem, why it matters, the considerations (options and their trade-offs), a "done when",
   and a first `## History` entry.
3. Run `issues/generate.sh`, or let the pre-commit hook run it.
4. Cross-reference related issues by number, e.g. `[19](19-memory-tiers.md)`, where one genuinely
   blocks or overlaps another.
