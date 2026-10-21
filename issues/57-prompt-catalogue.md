# 57 — Prompts are English constants spread through the code

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** The prompts the runtime gives models are English constants spread through the code, in one variant for every language and model.

## The problem

The texts the runtime gives models are string constants in the code, all in English: tool
descriptions (`note`, `note_private`, `read_history`, ...), the headers of prompt parts ("[Notes in
this channel ...]", "[Your private notes ...]"), `PLANNING_SYSTEM_PROMPT` and the plan step
instructions, the texts a model receives for refused or failed tool calls. They exist in one
variant, for every language ([56](56-response-language.md)) and every model.

The question raised: should these prompts exist in several languages, and perhaps in variants for
particular models? If so, where would such a collection be managed, and how would the best variant
for a model be chosen? **First to be discussed is whether this is sensible at all.**

## Why it matters

- English instructions pull answers towards English, especially in small models - one cause of
  [56](56-response-language.md).
- Models differ: small local models need shorter, more explicit prompts; some merge system messages
  ([49](49-plan-step-system-messages.md)); tool calling behaves differently. One text for all is a
  compromise for each.
- Texts in the code can only be changed by a release; an operator cannot tune them.

## Considerations

- **Against it.** Larger models follow English instructions at least as well as translated ones, and
  "answer in German" in an English prompt usually suffices; tool descriptions in English are the
  norm and what models are trained on. Every text in n languages times m model variants multiplies
  maintenance, and without evaluations nobody knows whether a variant is better or just different.
  Automatic matching of prompts to models is a prompt engineering platform of its own.
- **For it.** Small models demonstrably mix up or ignore instructions given in another language than
  the conversation; model-specific quirks exist and are today worked around in code. An operator
  being able to adjust texts without a release is useful independently of languages and models.
- **Where a collection would live**, options:
  - Constants in the code, as today.
  - Built-in texts become parameters of the components that use them, with the current English text
    as the default - the pattern `static_system_prompt_provider` already follows with `prompt`. The
    personality, which also chooses the model, overrides them where needed. No new infrastructure;
    variants per language or model are then simply different personalities or component
    configurations.
  - A prompt catalogue: resource files or entities in the `EntityStore`, keyed by text id, language
    and model selector, with a fallback chain (`de-CH` -> `de` -> `en`; model -> family -> default),
    overridable per deployment.
- **How to match a variant to a model**, if at all: explicit selectors in the catalogue (model name
  patterns, families, sizes) with the most specific match winning, like locale fallback; or no
  matching - the personality binds its texts and its model together. An automatic "best variant"
  needs evaluation data that does not exist.
- **Order.** Measure first whether a language prompt part ([56](56-response-language.md)) already
  solves the language problem with English instructions. The step that is useful in any case is
  making built-in texts overridable component parameters; a catalogue with languages and model
  selectors only if repetition across personalities proves painful.

## Decision

The texts move out of the code into a prompt catalogue, kept like translatable strings, so that a
text can be tuned for a model without a change of code. Decided by the maintainer on 2026-10-20,
after the experiments on mentions (`work/quiet_agents`) showed how much a model's behaviour hangs
on the wording and the place of a single sentence - and that today every finding has to be carried
by hand from the experiment's patched strings into constants of the code.

*   **What it is for, first: tuning per model.** Languages are a later dimension of the same
    catalogue, not the reason for it. A catalogue also shows every text side by side, where
    contradictions become visible - the roster telling agents to "mention one by their @handle to
    address them" and, one line below, to mention one only when they want its answer.
*   **The model's profile does not hold the texts; the model's component names a variant.**
    `ModelProfile` is a small closed set of rules, recorded with every message, and stays so. The
    component of the model - `ollama_client`, say - names the variant of texts its model gets
    (`prompt_texts: gemma4`), as it names its `sender_style`. A variant holds only the texts it
    changes; every other text is the default's. The choice is explicit: no patterns on model names,
    no automatic matching.
*   **What goes into it: all prose the runtime gives a model** - prompt parts and their titles,
    the descriptions of tools, of their parameters and of tool sets, what a model is told of a
    tool call refused, interrupted or held back, the plan's texts, the compaction's, the marking
    of senders and the notices of `caelumora-model`, the questions to the decision model. Not in
    it: the **names of tools**, which are identifiers that transcripts, matchers and approval rules
    hang on; and the **texts of errors** (`ToolError` and the error types whose `Display` a tool
    passes on), which are error types, also written to logs, under the rule that an error message
    names what failed. That a frontend shows errors in its user's language needs technical
    information beside their text - [443](443-errors-carry-only-an-english-text.md).
*   **Where it lives: a type of `caelumora-model`**, the lowest crate, since texts are written
    there too (`context.rs`). Every crate and plugin registers its keys and its default file
    (`include_str!`) when it registers its components, as it registers its kinds of component;
    plugins in a namespace of their own and add nothing to the core's catalogue
    ([PLUGINS.md](../docs/PLUGINS.md)). The catalogue is checked at startup: a key missing from a
    default, or an override that uses a variable its default does not have, keeps the runtime
    from starting rather than failing in the middle of a turn.
*   **The default is compiled in** (`include_str!`), so that the binary is complete without files
    of its own; variants and an operator's overrides come from the configuration. A language, with
    its fallback (`de-CH` -> `de` -> `en`), can later be added to the lookup beside the variant.
*   **An entry is a whole part, as a template with loops and conditions.** Most texts are put
    together - the roster from its participants, the plan from its steps, the catalogue of skills
    from its skills. With placeholders alone, the catalogue would hold fragments and the code would
    still decide how they are put together: a variant could change words, but not the structure -
    a list for prose, the rule before the roster -, and it is the structure that the experiments
    showed to matter. So an entry is a prompt part, a tool's description, a notice as a whole, and
    a variant can rewrite it as a whole. The engine is **`minijinja`** (Jinja2): few dependencies,
    no access to files or the network, a context given by `serde` or `context!{}` without view
    types of its own. `askama` compiles its templates and allows no overrides; `tera` and
    `handlebars` can do as much and weigh more; Fluent knows plurals and languages, but no loops.
    The file is YAML, the templates in block scalars (`|`).
*   **The code decides whether and with what, the template how it reads.** Whether a part is given
    at all and which data it gets is decided by the code; the template may branch and loop over
    those data - an empty list, a person or an agent -, but computes nothing: no filters or
    functions of our own, and what it needs to tell - whether a participant is an agent, which name
    serves as the example - the code hands it. Undefined variables are errors
    (`UndefinedBehavior::Strict`); whitespace is held by `trim_blocks`, `lstrip_blocks` and
    `keep_trailing_newline`. The variables of a key are those its default uses
    (`undeclared_variables`): an override may use only these, which is the check at startup, and
    the tests that compare prompts word for word check the defaults.
*   **How the catalogue reaches its users**: it is a service that prompt providers, tools and
    whoever else writes for a model get when they are made (`.uses::<…>()`) and keep as a field.
    Which variant applies they learn from the profile: `ModelProfile` names the variant chosen by
    the model's component, and since the profile is recorded with every message, so is which texts
    a model was given. The cost: `ModelProfile` is `Copy` today and becomes `Clone`. Carrying the
    resolved texts in the `Work` beside the profile instead was set aside: tools, the decision
    model and the `ContextResolver`, which write outside a turn, would need a second way. Tool
    descriptions are built in `ToolProvider::tools()`, which knows no `Situation`; in the first
    step they take the default, and their variants come with the second through `offered`.
*   **Steps.** First a refactoring without a change of behaviour: the texts move into the
    catalogue, and the tests that compare prompts word for word keep them the same. To stay
    reviewable it is cut into four commits: the catalogue with the prompt parts of
    `caelumora-agents`; the descriptions of tools; what a model is told of tool calls, the plan,
    the compaction and the texts of `caelumora-model`; the plugins - skills, workspace tools,
    artifacts, decision. Then the variants per model and the override from the configuration.
    Work on prompt texts in the meantime - such as the rule on mentions - is not held back: it goes
    into constants as today and moves with the rest.

The other options weighed above are thereby set aside: constants in the code cannot be tuned
without a release; texts as parameters of each component would scatter the overrides over every
personality and component, and repeat them in each.

## Done when

- It is decided whether prompts in the code are to exist in several languages and/or model variants,
  and why - or recorded as rejected, with the reason.
- If so: where they are managed and how a variant is chosen.

## History

- **2026-09-26** — Issue created at the maintainer's request, together with
  [56](56-response-language.md): should the prompts spread through the code be kept in several
  languages, perhaps model-specific, where would such a collection be managed, and how is the best
  variant for a model chosen - to be discussed first whether this is sensible at all.
- **2026-10-05** — From [56](56-response-language.md): the part telling an agent the language of its
  channel (`channel_language_prompt_provider`) is English, naming the language in English and in
  itself. Whether a part written in the channel's language holds a model to it more surely is open;
  if a comparison on the models in use shows it does, that variant belongs in this catalogue, not in
  constants beside the English one.
- **2026-10-20** — Decided (see "Decision"): a prompt catalogue with a compiled-in default and
  variants per model, named by the model's component; the type changes from `question` to
  `feature`. Prompted by the experiments on mentions in `work/quiet_agents`, where the wording of
  one rule decided whether agents kept mentioning each other. The issue is done when the texts are
  in the catalogue, a variant named by a model's component replaces single texts of the default,
  and [AGENT-TURN.md](../docs/AGENT-TURN.md) and [CONFIGURATION.md](../docs/CONFIGURATION.md)
  describe both.
- **2026-10-21** — The first step settled with the maintainer (see "Decision"): what goes into the
  catalogue and what not - the texts of errors became
  [443](443-errors-carry-only-an-english-text.md); the catalogue as a type of `caelumora-model`,
  filled by every crate and plugin at registration and checked at startup; entries as whole parts,
  templates of `minijinja` with loops and conditions instead of placeholders, at the maintainer's
  suggestion, so that a variant can change how a part is built and not only its words; the variant
  named in `ModelProfile`; the first step in four commits.
- **2026-10-21** — Step 1, commit 1: `PromptCatalogue` in `caelumora-model` (`minijinja` 2.24),
  filled through `RuntimeBuilder::prompts` and offered as a service once the runtime is built; the
  agents' texts in `caelumora-agents/src/prompts.yaml`, namespace `agents`, rendered by the prompt
  providers of `prompt/` and by the prompt parts of `notes`, `private_notes` and `agent_memory`,
  word for word as before. A template that does not parse stops the start; that the code gives every
  value a text names is checked by the tests, which render every part - the check of overrides
  against their defaults comes with them in step 2.
- **2026-10-21** — Step 1, commit 2: the descriptions of the agents' tools, their parameters and
  their tool sets are texts of the catalogue, under `tools.<kind>` in
  `caelumora-agents/src/prompts.yaml`. A provider renders them once, when it is made, and keeps its
  tools; `tool::described` gives a tool and its parameters the texts of their paths
  (`ToolParameters::describe`, prepared in a commit of its own: the schema is built without
  descriptions). Groups of texts in the catalogue's files, which are read with strict booleans.
  The texts were compared with those before, word for word, for every tool of the agents.
- **2026-10-21** — Step 1, commit 3: the namespace `core` (`caelumora-model/src/prompts.yaml`),
  added by `RuntimeBuilder::new` - the notices of the `ContextResolver`, which is given the
  catalogue (`ContextResolver::of`, `on_path`, `ModelProfile::given_messages`), and the
  explanation of how messages are marked. In `agents`: the results of tool calls, the plan's
  instructions, what was heard while working, the part on planning and the compaction's texts.
  Decided with the maintainer: the turn and the actor reach the catalogue through
  `ActorRuntime::prompts`; the notices are worded anew as a context is resolved, also for what an
  agent saw afterwards; what the model is told of why a turn was given up is the catalogue's
  (`core.given_up_cause`), while `GivenUpReason::cause` stays for logs and frontends
  ([443](443-errors-carry-only-an-english-text.md)); and a tool result whose message cannot be had
  is given with its status alone, logged, while one only its agent may see is withheld without its
  mark. Every text was compared with the one before, word for word.
