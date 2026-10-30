# Skills

A skill is a named, self-contained piece of procedural knowledge - "how we write a release note",
"how this schema is migrated" - that exists apart from any personality and is loaded into a turn
when a task calls for it. This document describes what a skill is and is not, where it lives, who
may use and write one, and how it reaches the model. Skills are a plugin, the crate
`caelumora-skills` (issue 323, see [PLUGINS.md](PLUGINS.md)): the core knows nothing
of them. How a command of an agent is carried out is in
[COMMANDS.md](COMMANDS.md#commands-of-an-agent); how far information may travel
in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## What a skill is

*   **Instructions only.** A skill tells the model how to do something with the tools it has. It
    brings no tools, no components, nothing that runs: the worst a skill can do is give bad advice,
    and what follows from the advice is bounded by the agent's tools, their approval and the rules
    of a place as everything else is.
*   **Not memory.** Memory holds what an agent knows, written by the agent, with the provenance of
    every entry ([MEMORY.md](MEMORY.md)). A skill holds how a task is done, belongs to a user, is
    read and changed by people as a document, and is shared like one.
*   **An entity** (`caelumora_skills::skill::Skill`), kept in the `EntityStore` like a workspace: a
    name unique in its scope - the global one, a user's (its owner's) or a session's, where an
    agent made it and with which it is deleted, restored and purged; no channel's, since a skill
    serves a session's channels alike. A one-line `description` is what the catalogue shows the
    model, which decides by it whether to load the skill; the `body` is what the model is given
    when it is loaded. One classification holds for all of it. Its owner is a user, or the system
    for a skill of the seed, which lies in the global scope alone.
*   **No versions.** What was loaded stands in the transcript word for word, with its digest in
    the knowledge of the turn; a skill changed afterwards is loaded anew next time.

## Who may do what

Two questions, apart:

*   **Managing** a skill is asked as for every entity: `GetSkill`, `List(skill)`, `CreateSkill`,
    `UpdateSkill`, `DeleteSkill`. Under the ownership policy its owner may do everything, whom its
    access list grants reading read it, a skill of the system is everybody's to read. A user is
    shown a skill only if cleared for its classification, as a channel. Making or reclassifying one
    asks the owner's clearance, as for a workspace, so that nobody writes what they could not read.
*   **Using** a skill - loading it into a turn - is `UseSkill`, the skill's own verb `skill::USE`,
    apart from reading it. It is asked in the turn's context: the agent for its owner, whoever asked
    as initiator
    ([SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)). The ownership policy grants it as
    reading: what the owner may read, the agent may follow. Besides, the skill's classification must
    flow into the channel (`may_publish`): a skill classified above the channel is not loaded there.

The skills an agent may load where it is are `SkillViews::usable_skills`: of those it may read, each
it may use whose classification flows into its channel, and of those, for every name, the one found
from its channel - the session's before the owner's before the global one. Filtered first and chosen
after, so a skill of the session the agent may not use does not hide the global one of its name. One
that may not be used is answered as one that does not exist.

## How a skill reaches the model

Through the agent's own transcript, and nowhere else: not through the system prompt - that would
break the prompt cache from there on and leave nothing in the record of what the model saw but a
prompt part -, and not as a message of the channel's history, which would hand its text to
participants who may not read the skill.

*   **The catalogue** is a prompt part, `[Skills you can load]`: a line per usable skill, its name
    and description, and how to load one. In the system prompt by default: it changes rarely, and
    the model has to know what it could load. Classified as the bounded join of the skills it lists;
    nothing where there are none.
*   **`load_skill(name)`** - the model loads one. Its result names the skill - classified as the
    skill, as the catalogue names it - and the skill's text is given to the agent as a `/skill`
    gives it (`Turn::give`), after the results of the step: in the agent's name, as a `Given` entry
    that names the skill (issue 389). The channel sees which skill was loaded, not what it says,
    and the model is told not to quote it.
*   **`/skill @agent <name>`** - a participant gives one to an agent, a command the agent carries
    out itself. In its command turn the skill's text is given to the agent (`Turn::give`): into its
    transcript in the participant's name and role, as a `Given` entry that names the skill. The
    channel's history records the command, and the text as what the agent was given, gone to
    nobody but read by whom may read what the agent saw (issue 417).
*   **`list_skills`** gives the catalogue as a tool, for a model whose prompt was built before a
    skill appeared; **`/skills @agent`** lists, privately, what the agent may load here - for its
    owner, as it answers for its owner, so the issuer learns the names and descriptions of the
    owner's skills.

Every load is recorded as knowledge, and the turn has seen the skill: what it writes afterwards is
classified at least so.

A compaction gives again the skills loaded in the part of the transcript it replaces - given or
loaded by the model, either way a `Given` entry - that the work still open needs, as the agent's
model answers after its summary (issue 413): as each is now and only if the agent may still load
it, unless the part kept still holds it. A skill of a task that is over is not given again; it
stays in the catalogue, to be loaded again. The compaction asks the `skills` component for them, as
its `KnowledgeRestorer` (issue 323): it knows no skills itself.

## Agents write skills

An agent may write down what it has learnt to do:

*   **`create_skill(name, description, body, scope)`** - `scope` is `session`, the turn's session,
    or `user`, the owner's; never the global one. The skill is its owner's - `CreateSkill` asked
    for the owner -, classified as the turn's label.
*   **`update_skill(name, description?, body?)`** - one the agent may load here and its owner owns:
    a skill shared with the owner is loaded, not changed. The skill is first raised to what the
    changing turn has seen, then its text changes - otherwise a turn that saw something
    confidential could write it into an internal skill.
*   **Approved** unless the component says otherwise (`requires_approval`): the approver is shown
    the skill whole - its name, where it goes, its description and its steps.

The tools' texts tell the model the rules: a skill is for what to do, not for what one knows;
check the skills one can load first, and update one rather than writing a second.

## The component

`skills` is the one component of skills, as `notes` is of notes: the catalogue, the tools and the
commands above (see [COMPONENTS.md](COMPONENTS.md) for its parameters). An agent without it knows no
skills. It works for agents run here and external agents alike; an external agent gives a skill
given to it to its remote agent with its next call.

## Where skills are written

*   **The `SkillManager`** (`caelumora_skills::manager`) is the one writer: it makes, changes,
    deletes, restores and purges skills - stored first, then announced to whoever may read the
    skill. The plugin offers it as a service of the runtime; an agent's tools reach it through
    their views (`SkillViews::skill_manager`), the routes when they are built.
*   **REST** lets a user manage skills (see [REST-SERVER.md](REST-SERVER.md)); one of a session's
    scope is written by an agent in it.
*   **The web UI** lists them and shows one on a page of its own, where its owner edits it (see
    [WEB-UI.md](WEB-UI.md#skills)).
*   **A seed** may hold skills of the system in its section `skills`, which the plugin claims (see
    [CONFIGURATION.md](CONFIGURATION.md)).

## Where the code lives

The crate `caelumora-skills` holds all of it, in layers by feature (issue 323, cut 11): without
features the entity, its body, digest and snapshot (`skill`) and the wire types (`api`), which a
client reads; `runtime` adds the `SkillManager` (`manager`), the views as an extension trait on the
runtime's `Views` (`view::SkillViews`), the questions of who may change and use a skill as an
extension trait on the `AccessPolicy` (`access::SkillAccess`), the `skills` component
(`component`) and `SkillsPlugin` (`plugin`), which joins them to the runtime; `server` adds the
routes and how a `SkillError` is answered (`routes`). Its tests run in the world of the security
suite (`tests/skills.rs`). `caelumora-app` lists the plugin with its routes, `caelumora-tui` with
the feature `runtime` alone.

## Not decided yet

*   A personality that loads a skill in every turn (preloading), and importing `SKILL.md` files.
*   Resources beside a skill - files, examples -, which would be the workspace's.
