# 301 — The skills an agent lists are shown as raw JSON

**Status:** done
**Area:** User interaction

## The problem

`load_skill`, `create_skill` and `update_skill` have a presentation of their own, `skill` (issue
[281](281-tool-argument-visibility.md)): the web UI shows the skill's name - linked to its page
where the answer holds its id -, what it is for and its text. `list_skills` is presented as every
call (`ToolPresentation::Default`): no arguments, and its result as the JSON object the tool
returns, `{ "skills": [{ "name", "description" }] }`.

281 decided the presentation for the three tools that act on one skill and left the catalogue
aside; nothing decided that it should stay raw.

## Why it matters

- An agent that has skills calls `list_skills` before it loads one, so the call is common, and as
  JSON it is hard to read: which skills, and what each is for, is buried in field names and quotes.
- The skill tools are shown unevenly - three as what they do to a skill, one as data - as the file
  tools were before issue [265](265-file-list-and-search-presentations.md).
- What the same catalogue looks like elsewhere is already readable: the command `/skills` answers
  with a list, each skill's name as code and what it is for.

## Considerations

The pattern is set: a tool declares its presentation, the web UI's `ToolCallView` has a handler
for each (`PRESENTATIONS`), and a client that does not know one falls back to the default.

- **Its own presentation or `skill` extended.** 281 chose one presentation for the three skill
  tools because each shows *one* skill, read from fields rather than tool names. A catalogue is a
  list of skills - as a listing of the workspace is not a file read. Proposed: a variant of its
  own, `ToolPresentation::SkillList` (`skill_list`), named after `file_list` beside `file_read`.
  Extending `skill` to read a `skills` array as well would save a variant, but give one handler
  two shapes to tell apart. *To decide.*
- **What the handler shows:** how many skills the agent may load, and each with its name and what
  it is for, as `/skills` lists them; none, said as such ("No skills to load here").
- **A link to each skill.** The other skill presentations link the name to `SkillPage` where the
  answer holds a `skill_id`. The catalogue's entries hold none. Proposed: each entry takes its
  `skill_id`, as the answers of `create_skill` and `update_skill` already do, and the handler links
  the name; `SkillPage` answers on its own when the viewer may not read the skill. The cost is an
  id per entry in what the model is given - a few tokens it has no use for. Without it the names
  stay text. *To decide.*
- **Visibility.** `list_skills` stays `SHOWN`; its result is the owner's where the frame says so
  ("not shown") as for every call, and a failed call shows its message.
- The test that lists which skill tools are presented as `skill`
  (`load_skill_shows_the_channel_which_skill_it_loaded_not_what_it_gave`) is extended by the
  catalogue's presentation.

## Done when

`list_skills` has a presentation of its own, the web UI shows the skills listed by name and what
each is for (linked to their pages, if so decided), and `TOOLS-AND-COMMANDS.md` and `WEB-UI.md`
say so.

## History

- **2026-10-10** — Issue created at the maintainer's request: `list_skills` has no presentation
  in the web UI. Proposal above; whether it is a variant of its own and whether the entries carry
  their ids are still to decide.
- **2026-10-10** — Decided: a variant of its own, `ToolPresentation::SkillList` (`skill_list`),
  and each entry carries its `skill_id`, as the answers of `create_skill` and `update_skill` do -
  only the runtime knows which skill a name resolves to where the agent is, so a frontend could not
  find the id itself. Done: `list_skills` answers `{ "skills": [{ "name", "description",
  "skill_id" }] }` and is presented as `skill_list`; the web UI's `SkillListToolCallContent` shows
  how many skills there are ("No skills to load here" for none), and each by its name, opening its
  `SkillPage` as the `skill` handler does, and what it is for; "Skills not shown" where the result
  is the owner's, and the message of a failed call. Tests: the catalogue's result holds each skill
  with its id, and `list_skills` is presented as `skill_list` and `SHOWN`. TOOLS-AND-COMMANDS.md,
  WEB-UI.md, SKILLS.md. Nothing stored changes.
