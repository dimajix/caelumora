# 532 — Component kinds are named after their facet

**Status:** in progress
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Most component kinds of an agent are named after the facet they serve - `image_tool`, `terminal_tool`, `current_time_prompt_provider` - while those of several facets are named after what they give the agent - `skills`, `notes` -, so that a personality says through which facet an agent gets something rather than what it gets, and a kind of several facets is named after one of them or after none.

## The problem

A personality lists the components an agent is built of, each by its kind. Two schemes name them:

- **After the facet:** `calculator_tool`, `ask_user_tool`, `stop_tool`, `attachment_tool`,
  `create_plan_tool`, `mcp_tool`, `artifact_tool`, `image_tool`, `terminal_tool`, and the prompt
  parts `static_system_prompt_provider`, `channel_participants_prompt_provider`,
  `current_time_prompt_provider`, `workspace_prompt_provider`, ….
- **After what they give the agent:** `skills`, `notes`, `private_notes`, `agent_memory`,
  `channel_history`, `compaction`.

COMPONENTS.md draws the line as a convention ("Writing a component"): a `_tool` kind has a struct
`…Provider`, a *feature* - a component of several facets - is named after its subject, with a struct
`…Component`. The line is not kept: `create_plan_tool` provides a tool and a prompt part and is
named as a tool; `channel_history` provides a tool alone and is named as a feature.

And the facet does not say what a component brings. A component is built whole, and its owner takes
every facet it asks for ("A kind is accepted if one of its facets is in the closure"): who writes
`skills` into a personality gets the tools on skills, the prompt part that lists them, the command
`/skill @agent <name>` and the `KnowledgeRestorer` that `compaction` takes. A name after one of
these facets - `skill_tool` - would hide three of four.

Raised by the maintainer on 2026-11-02, in the discussion of issue
[531](531-workspace-tools-named-apart-from-the-tools.md).

## Considerations

- **The rule** (decided 2026-11-02 for the tools): a component kind is named after what it gives
  the agent, never after its facet. The tool/feature distinction of COMPONENTS.md goes with it.
- **The names of the tools' kinds** (decided 2026-11-02):

  | today | renamed |
  |---|---|
  | `image_tool` | `image_generation` |
  | `terminal_tool` | `workspace` |
  | `mcp_tool` | `mcp_server` |
  | `artifact_tool` | `artifacts` |
  | `attachment_tool` | `attachments` |
  | `create_plan_tool` | `planning` |
  | `ask_user_tool` | `ask_user` |
  | `stop_tool` | `turn_end` |
  | `calculator_tool` | `calculator` |
  | `sleep_tool`, `echo_tool` | `sleep`, `echo` |

  `skills`, `notes`, `private_notes`, `agent_memory` and `channel_history` keep their names. The
  names of the tools the model sees - `generate_image`, `load_skill`, … - do not change.
- **`workspace` takes the prompt part of the workspace.** `workspace_prompt_provider` names
  `read_file`, a tool of `terminal_tool`, and tells nothing true without it; issue
  [428](428-workspace-tools-in-a-crate-of-their-own.md) kept the two kinds apart only so that no
  configuration changed, and found them to belong together by subject. One kind `workspace`
  provides both facets, as `skills` provides its prompt part; its configuration holds the
  parameters of both (`max_wait_secs`, `max_output_bytes`, `requires_approval`, `description`,
  `position`, `instruction_files`).
- **The other suffixes of facets** (recommended, to be decided): the rule holds for the prompt
  parts and context providers as for the tools - `system_prompt`, `channel_participants`,
  `channel_language`, `channel_briefing`, `current_time`, `request_reason`, `channel_mentions`,
  `tool_sets`, `turn_context`, `external_context` -, and for the commands - `help`, `wakeup`,
  `nudge`. Renaming only the tools would leave two schemes beside each other again. Not
  concerned: `language_model`, `image_model`, `a2a_agent`, and the moderator and its policies
  (`policy_moderator`, `…_policy`, `decision_policy`), whose suffix names what the thing is, not a
  facet it serves.
- **The structs** follow the kinds; the convention of COMPONENTS.md for them - `…Provider` for a
  tool, `…Component` for a feature - is replaced by one rule, to be laid out in the plan.
- **Checked by a test:** the registry of the programs' libraries holds no kind whose name ends in a
  suffix of a facet (`_tool`, `_prompt_provider`, `_context_provider`, `_command`).
- **Stored data:** the kinds are named in the stored personalities and component bundles, and in
  `examples/seed.yaml`. One that names a renamed kind is refused when its agent is built
  (`ComponentError::UnsupportedKind`); no code keeps the old names. The personalities and bundles
  that name one are deleted, so that the seed writes them again. Sessions and transcripts are not
  concerned: a tool call carries the name of its tool, not the kind of its component.

After issue [531](531-workspace-tools-named-apart-from-the-tools.md), which renames the crates of
the tools (`caelumora-tool-workspace`, `caelumora-tool-mcp`).

## Done when

- Every component kind of an agent is named after what it gives the agent, the workspace's tool
  and prompt part are one kind, and a test refuses a kind named after its facet.
- COMPONENTS.md, TOOLS.md and the other documents, `examples/seed.yaml` and `examples/config.yaml`
  name the kinds so.

## History

- **2026-11-02** — Opened at the maintainer's request, in the discussion of issue
  [531](531-workspace-tools-named-apart-from-the-tools.md).
- **2026-11-02** — Decided with the maintainer: the rule holds for the prompt parts, the context
  providers and the commands as well; every struct of a component kind is named `…Component`, its
  configuration `…Config`, its constant `…_KIND` and its file after the kind; the texts of a kind
  are keyed by it. Two steps: first `terminal_tool` and `workspace_prompt_provider` become one kind
  `workspace`, then every other kind is renamed, with the rule in COMPONENTS.md and its test.
- **2026-11-02** — Step 1: `terminal_tool` and `workspace_prompt_provider` are the one kind
  `workspace` (`WorkspaceComponent`, `WorkspaceConfig`, in `workspace.rs` of
  `caelumora-tool-workspace`), which provides the tools and the prompt part; its texts are keyed
  `tools.workspace`.
