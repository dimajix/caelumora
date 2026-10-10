# Skills

A skill (issue [22](../issues/22-skill-management.md)) is a named, self-contained piece of
procedural knowledge - "how we write a release note", "how this schema is migrated" - that exists
apart from any personality and is loaded into a turn when a task calls for it. This document
describes what a skill is and is not, where it lives, who may use and write one, and how it reaches
the model. How a command of an agent is carried out is in
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#commands-of-an-agent); how far information may travel
in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## What a skill is

*   **Instructions only.** A skill tells the model how to do something with the tools it has. It
    brings no tools, no components, nothing that runs: the worst a skill can do is give bad advice,
    and what follows from the advice is bounded by the agent's tools, their approval and the rules
    of a place as everything else is.
*   **Not memory.** Memory holds what an agent knows, written by the agent, with the provenance of
    every entry ([MEMORY.md](MEMORY.md)). A skill holds how a task is done, belongs to a user, is
    read and changed by people as a document, and is shared like one.
*   **An entity** (`caelumora_model::skill::Skill`), kept in the `EntityStore` like a workspace:
    *   a `ScopedName` - a name unique in its scope (issue 250), the scope the global one, a user's
        (its owner's) or a session's, made there by an agent and deleted, restored and purged with
        the session; no channel's, since a skill serves a session's channels alike;
    *   a `description`, one line of at most 300 characters: what the catalogue shows the model,
        which decides by it whether to load the skill;
    *   a `body`, Markdown of at most 32 000 characters (`SkillBody`): what the model is given when
        the skill is loaded;
    *   one `classification` for all of it;
    *   `SecurityAttributes`: its owner - a user, or the system for a skill of the seed, which lies
        in the global scope alone - and the contributors it is shared with;
    *   when it was made and changed, and `deleted` (issue 164).
*   **No versions.** What was loaded stands in the transcript word for word, with its digest in
    the knowledge of the turn; a skill changed afterwards is loaded anew next time.

## Who may do what

Two questions, apart (issue 22, decision 2):

*   **Managing** a skill is asked as for every entity: `GetSkill`, `ListSkills`, `CreateSkill`,
    `UpdateSkill`, `DeleteSkill`. Under the ownership policy its owner may do everything, its
    contributors read it, a skill of the system is everybody's to read. A user is shown a skill
    only if cleared for its classification, as a channel (`Views::cleared_for`). Making or
    reclassifying one asks `Classify` and the owner's clearance, as for a workspace, so that nobody
    writes what they could not read.
*   **Using** a skill - loading it into a turn - is `UseSkill`, asked in the turn's context: the
    agent for its owner, whoever asked as initiator ([SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)).
    The ownership policy grants it as reading: what the owner may read, the agent may follow.
    Besides, the skill's classification must flow into the channel (`Publish`, as what a turn
    writes is asked): a skill classified above the channel is not loaded there.

The skills an agent may load where it is are `Views::usable_skills`: of those it may read, each it
may use whose classification flows into its channel, and of those, for every name, the one found
from its channel (`scope::found_from`) - the session's before the owner's before the global one.
Filtered first and chosen after, so a skill of the session the agent may not use does not hide the
global one of its name. `usable_skill_named` finds one; one that may not be used is answered as one
that does not exist (`ViewError::NoSkillNamed`).

## How a skill reaches the model

Through the agent's own transcript, and nowhere else (issue 22, decision 3): not through the system
prompt - that would break the prompt cache from there on and leave nothing in the record of what
the model saw but a prompt part -, and not as a message of the channel's history, which would hand
its text to members who may not read the skill.

*   **The catalogue** is a prompt part, `[Skills you can load]`: a line per usable skill, its name
    and description, and how to load one. In the system prompt by default (parameter `position`):
    it changes rarely, and the model has to know what it could load. Classified as the bounded join
    of the skills it lists (`join_within` the turn's classification); nothing where there are none.
*   **`load_skill(name)`** - the model loads one: the result is the skill's text as plain text
    under `[Skill: <name>]`, classified as the skill. Its result is the owner's (`WITHOUT_RESULT`,
    issue 281): the channel sees which skill was loaded, not what it says, and the model is told
    not to quote it.
*   **`/skill @agent <name>`** - a member gives one to an agent, a command the agent carries out
    itself (issue [269](../issues/269-agent-commands.md)). In its command turn the skill's text goes
    into its transcript as a message in the member's name and role (`Turn::give`, which records a
    message the agent is given to read, not one it said - into the transcript as a `Given` entry
    that names the skill, and into its history, unpublished). The channel's history records the
    command, not the text.
*   **`list_skills`** gives the catalogue as a tool, for a model whose prompt was built before a
    skill appeared; **`/skills @agent`** lists, privately, what the agent may load here - for its
    owner, as it answers for its owner (issue 255), so the issuer learns the names and descriptions
    of the owner's skills.

Every load is recorded as knowledge (`KnowledgeSource::Skill { skill, digest }`), and the turn has
seen the skill: what it writes afterwards is classified at least so (issue 168).

A compaction by summary ([16](../issues/16-context-compression.md), `/compact`, the strategy
`summary_compaction`) gives again every skill loaded in the part of the transcript it summarises -
given (`Given` entries) or loaded by the model (the knowledge recorded with what the agent said
afterwards) - as it is now and only if the agent may still load it, unless the part kept still holds
it. A skill changed since is given as it reads now, as a load would give it.

## Agents write skills

An agent may write down what it has learnt to do (issue 22, decision 6):

*   **`create_skill(name, description, body, scope)`** - `scope` is `session`, the turn's session,
    or `user`, the owner's; never the global one. The skill is its owner's - `CreateSkill` asked
    for the owner -, classified as the turn's label: what the turn had seen, at most the channel's
    classification. An agent of the system writes none: a skill belongs to a user.
*   **`update_skill(name, description?, body?)`** - one the agent may load here and its owner owns
    (`UpdateSkill`: a skill shared with the owner is loaded, not changed). The skill is first
    raised to what the changing turn has seen (`join_within` the turn's classification), then its
    text changes - otherwise a turn that saw something confidential could write it into an
    internal skill.
*   **Approved** unless the component's parameter `requires_approval` is `false`: the approver is
    shown the skill whole - its name, where it goes, its description and its steps.

The tools' texts tell the model the rules: a skill is for what to do, not for what one knows;
check the skills one can load first, and update one rather than writing a second.

## The component

`skills` (`component_impl/skills.rs`) is the one component of skills, as `notes` is of notes: the
catalogue (`PromptProvider`), `list_skills`, `load_skill`, `create_skill`, `update_skill`
(`ToolProvider`), `/skill` and `/skills` (`AgentCommandProvider`). An agent without it knows no
skills. Parameters: `position` (`start` or `before_turn_input`, default `start`) and
`requires_approval` (`true`, the default, or `false`). It works for agents run here and external
agents alike; an external agent gives a skill given to it to its remote agent with its next call.

## Where skills are written

*   **The `SkillManager`** (`caelumora_runtime::skill`) is the one writer: it makes, renames,
    describes, rewrites, reclassifies, deletes, restores and purges skills - stored first, then
    announced (`GlobalEvent::Skill…`, delivered to whoever may read the skill). The
    `RuntimeManager` holds it; an agent's tools reach it through their views.
*   **REST** lets a user list, read, make, change, delete, restore and purge skills (see
    [REST-SERVER.md](REST-SERVER.md)); one of a session's scope is written by an agent in it.
*   **The web UI** lists them in the sidebar and shows one on a page of its own, where its owner
    edits it (see [WEB-UI.md](WEB-UI.md#skills)).
*   **A seed** may hold skills of the system (`skills:`, see [CONFIGURATION.md](CONFIGURATION.md)).
    The example seed has `quiz-round` and `write-agents-md`, which guides an agent through
    exploring a project in its workspace and writing an `AGENTS.md` for it (issue 279); a
    deployment without that seed has neither.

## Not decided yet

*   A personality that loads a skill in every turn (preloading), and importing `SKILL.md` files.
*   Resources beside a skill - files, examples -, which would be the workspace's.
