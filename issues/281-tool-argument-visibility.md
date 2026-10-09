# 281 — A tool's arguments share one visibility flag with its result, so a call cannot publish what it was asked and hide what it returned

**Status:** open
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

## History

- 2026-10-09: Opened at the maintainer's request, after a call of `load_skill` in a local session
  showed neither which skill was loaded nor - to the agent's owner in the chat - anything more.
