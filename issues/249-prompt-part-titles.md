# 249 — Some prompt parts have no title, others one that is a paragraph

**Status:** done
**Area:** Context and knowledge
**Summary:** Some prompt parts have no title and run into the others, and some have one that is a paragraph.

## The problem

An agent's prompt is made of the parts its prompt providers contribute, joined by a blank line
(`TurnContextProvider`). Some parts begin with a short title in brackets - `[Who is in this
channel]`, `[Language of this channel]`, `[About this channel]` - and some do not: the planning
instructions of `create_plan_tool`, the unavailable tools of `tool_sets_prompt_provider`, and the
personality's own text from `static_system_prompt_provider` run into the parts around them. The
notes, the private notes and the memory have a title that is a whole explanation in brackets -
`[Your private notes - no other participant sees them; only the person responsible for you can
inspect them. Do not quote or reveal them in your answers.]`.

Each provider writes its title by hand (`format!("[…]\n…")`); only `render_sections` is shared, and
it takes any preamble.

## Why it matters

The prompt reads as one text to the model and to whoever looks at it in the context panel. Parts
without a title blur into each other, and a title that is a paragraph is no title. One form, made
in one place, keeps the parts apart and keeps new providers from inventing a third form.

## Plan

Decided with the maintainer on 2026-10-06.

- **One form:** a short title in brackets on a line of its own, then the explanation as plain text,
  then the content. `context::titled(title, text)` makes it - nothing if the text is empty - and
  every provider uses it; `render_sections` takes the title and the preamble apart.
- **The titles:** `[Planning]` for `create_plan_tool`, `[Unavailable tools]` for
  `tool_sets_prompt_provider`, `[Notes in this channel]`, `[Your private notes]` and `[What you
  remember]`; the explanations stay as they are, below them. The parts that have short titles keep
  them.
- **`static_system_prompt_provider`** gets an optional parameter `title`, without a default: it is
  used for any text, some of which carry a title of their own. The seed's personalities set
  `title: Who you are`.

## Done when

Every prompt part the runtime writes begins with a short title made by `context::titled`, the
static system prompt where it is configured to, and `AGENT-TURN.md` says so.

## History

- **2026-10-06** — Opened and planned with the maintainer after a look at all prompt providers.
- **2026-10-06** — Done, as planned. `context::titled` and `render_sections(title, preamble,
  sections)`; the roster, the language, the descriptions, the notes, the private notes, the memory,
  the unavailable tools and the planning instructions are titled by them, and
  `static_system_prompt_provider` reads `title` (`StaticSystemPromptProvider::of_params`, so that
  its parameters can be tested). External agents are given the same parts, each on its own, with
  nothing the runtime adds; as their texts changed, each is given to them once more. The seed's
  personalities set `title: Who you are`. Tests: a part titled and an empty one none, sections under
  title and preamble, the static prompt titled only where a title is named, the planning part under
  its title. AGENT-TURN.md, COMPONENTS.md.
