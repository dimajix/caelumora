# 281 — A tool's arguments share one visibility flag with its result, so a call cannot publish what it was asked and hide what it returned

**Status:** done
**Area:** Tools and approvals

## The problem

`ToolVisibility` (`caelumora-model/src/tool.rs`, issues 54, 133) has two flags: `call` - whether
the call is published at all - and `content` - whether its arguments *and* its result are. A tool
can publish both, neither, or only the call, but not the arguments without the result.

`load_skill` is the only `CALL_ONLY` tool. Its argument is the name of a skill, which `list_skills`
shows to everyone anyway. Only the skill's text, the result, is meant for the agent alone, as the
tool's description tells the model ("The channel sees that you loaded it, not what it says").
`Message::published` nevertheless sets the arguments of every call with `owner` content to `null`.
So the channel sees "load_skill" without a name. The agent's owner sees nothing more in the chat
either, because the chat shows the published form, which is all the channel history holds. Only
the context panel and the agent's history panel (`ReadAgentContext`) show the call in full.

Seen in a local session on 2026-10-09: after `/skill write-agents-md` and "write an AGENTS.md",
the chat showed Jonathan's call of `load_skill` with no arguments and an empty result. The
agent's owner could not tell which skill was loaded.

## Why it matters

- **It hides more than it protects.** The name of a skill is not secret: it is in the catalogue,
  and when the user loaded it with `/skill`, it is already in the channel as the command's
  arguments.
- **The reasoning is withheld as well.** A message's reasoning is the owner's when a call it leads
  to has `owner` content (CONFIDENTIALITY.md, *Reasoning*), because reasoning comes before the
  call and may hold what goes into its arguments. Since the result counts too, the reasoning
  before every `load_skill` call is hidden, though nothing in it can stem from the result.
- **The chat cannot say what the agent did.** "Loaded a skill" without saying which one is of
  little use to anyone following the channel, its owner included.

## Considerations

**1. Split `content` into `arguments` and `result`.** `ToolVisibility { call, arguments, result }`,
with the rule that neither is published where the call is not. `CALL_ONLY` keeps its meaning
(both `owner`), and a new constant - e.g. `RESULT_HIDDEN` - publishes call and arguments;
`load_skill` takes it.
- `Message::published` redacts arguments and result each by its own flag.
- The reasoning rule follows `arguments`, not the result: only what goes into a call can be
  anticipated by the reasoning before it.
- `tool::successful_result` (the "only you see this result" wrapper) follows `result`.
- The web UI (`ToolCallView`, `ToolSetList`) and the REST shape of `visibility` change - stored
  messages carry the old shape and would have to be deleted (no compatibility code, AGENTS.md).
- Costs: a third field everywhere the flags are stamped, compared and shown; one more rule to keep
  (no flag wider than `call`).

**2. Redefine `CALL_ONLY` as "call and arguments shown, result hidden".** Two flags stay, but
`content` would mean only the result, and the arguments would always follow `call`.
- Smallest change, and the only user of `CALL_ONLY` wants exactly that.
- But a tool can then no longer hide its arguments while showing that it was called. That is the
  case where the arguments carry what is private - a search in private knowledge, a query
  naming a person -, which `HIDDEN` covers only by also hiding that anything happened.

**3. Leave the flags, show the owner the full call in the chat.** The published form stays as it
is; a frontend fetches the agent's own view for messages whose owner reads them.
- Solves only the owner's view, not the channel's, and the reasoning stays withheld.
- A second path past the redaction, which today happens in one place only.

**Recommendation:** option 1. It names what the two halves of a call are, and the reasoning rule
can then follow what the reasoning can actually anticipate. Option 2 saves a field but loses a
distinction a future tool needs. Option 3 is a separate question (whether the owner should see the
unredacted form in the chat), not an answer to this one.

### A presentation for `load_skill`

Asked for with this issue: `load_skill` has the presentation `default`, so a call is shown as its
name, its raw arguments (`{"name": "write-agents-md"}`) and its result as plain text - where it
is seen at all, in the context and history panels, the skill's Markdown as one block with its
`[Skill: …]` title. A presentation of its own (`ToolPresentation::SkillLoad`, `skill_load`, issue
165) would show:

- **to everyone the call reaches:** that a skill was loaded and which one, by its name - which
  depends on the arguments being published (option 1 or 2 above); under today's flags it could
  only say "a skill was loaded";
- **to whoever sees the result** - the owner, in the context and history panels: the skill's text
  rendered as Markdown, as `SkillPage` renders it, without the title line the model is given.

Questions to settle with it:

- **What the handler reads.** The result is plain text for the model (`skills.rs`, `loaded`),
  where other presentations read fields of a JSON result. Either the handler takes the name from
  the arguments and drops the result's first line - which ties it to the text's form - or the
  result becomes a JSON object (`name`, `body`) that the model is given as well. Plain text was
  chosen for the model; that should not change for the sake of a display.
- **A link to the skill.** The call names the skill only as the agent found it by name in its
  scope; a frontend cannot tell which skill that was, nor whether the viewer may read it. A link to
  `SkillPage` needs the skill's id in what the viewer sees - in the published arguments it is not.
  Without it, the name stands as text.
- **`create_skill` and `update_skill`.** They show a skill's name, description and text as raw
  arguments too; the same handler, or a sibling, could show them as a skill.

## Decision

By the maintainer (2026-10-09), first on the presentation (1 - 3), then on the flags and the form
(4, 5).

1. **`load_skill` shows which skill was loaded, by its name, and not its text** - not to the owner
   either. The result stays plain text for the model, as it is.
2. **`create_skill` and `update_skill` show the skill's new text**, rendered as Markdown, with the
   name and the description; `update_skill` what it changes - description, text or both.
3. **A link to the skill where the answer holds its id.** The answers of `create_skill` and
   `update_skill` are JSON objects that are published (`SHOWN`); they take the skill's id
   (`skill_id`), and the handler links the name to `SkillPage` - which answers on its own when the
   viewer may not read the skill, or it is gone. `load_skill`'s answer is the model's plain text,
   and the channel does not see it, so its name stays text.

**Recommendation for the form:** one presentation, `ToolPresentation::Skill` (`skill`), for all
three tools rather than one each. It shows the name from the arguments, the description and the
text where the arguments have them, and the link where the result has a `skill_id`; `load_skill`,
whose arguments hold only the name, is shown as just that. Like the file presentations, it reads
fields, not tool names.

4. **Option 1:** `ToolVisibility` has three flags, `call`, `arguments` and `result`; the new
   constant `WITHOUT_RESULT` publishes call and arguments, and `load_skill` takes it.
5. **One presentation, `skill`,** for `load_skill`, `create_skill` and `update_skill`, as
   recommended.

## Done when

- A tool can declare that its call and arguments are published while its result stays with the
  agent and its owner, and `load_skill` does.
- `Message::published` redacts arguments and result each by its flag; the reasoning before a call
  is withheld for its arguments, not for its result.
- The web UI shows the published arguments of such a call and marks its result as shown to the
  owner only; the tools panel and context panel mark the flags.
- Tests cover the redaction of each combination and the reasoning rule; CONFIDENTIALITY.md and
  TOOLS-AND-COMMANDS.md (table of built-in tools) describe the three flags. The summary names the
  stored data to delete.
- The skill tools have a presentation of their own: `load_skill` shows the name of the skill it
  loaded and not its text; `create_skill` and `update_skill` show the name, the description and
  the new text as Markdown, and their answers hold the skill's id, which the web UI links to the
  skill's page. WEB-UI.md and TOOLS-AND-COMMANDS.md describe it.

## History

- 2026-10-09: Opened at the maintainer's request, after a call of `load_skill` in a local session
  showed neither which skill was loaded nor - to the agent's owner in the chat - anything more.
- 2026-10-09: Extended at the maintainer's request by a presentation of its own for `load_skill`.
- 2026-10-09: Decided for the presentation (see *Decision*): `load_skill` shows the name alone,
  `create_skill` and `update_skill` the new text, and a link where the answer holds the skill's id.
- 2026-10-09: Decided option 1 and one presentation for the three skill tools.
- 2026-10-09: Done. `ToolVisibility { call, arguments, result }` with `WITHOUT_RESULT` beside
  `SHOWN`, `CALL_ONLY` and `HIDDEN`; `Message::published` redacts arguments and result each by its
  flag, the reasoning rule follows `arguments`, `tool::successful_result` follows `result`.
  `load_skill` is `WITHOUT_RESULT`; the skill tools have the presentation `skill`, and
  `create_skill` and `update_skill` answer with the `skill_id`. The web UI's `skill` handler shows
  the name - linked to `SkillPage` in the chat, through `onSelectSkill` -, the description and the
  text as Markdown; what is the owner's is marked by one helper, `ownerOnly`, in the chat and the
  tool sets. Stored messages and tool sets with a visibility other than `SHOWN` carry the old
  shape and have to be deleted.
