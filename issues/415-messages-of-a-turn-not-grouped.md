# 415 — The chat does not show which messages belong to the same turn

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The chat shows every message of an agent as a bubble of its own, named and spaced like any other, so a turn of several messages cannot be told apart from several turns one after the other.

## The problem

A turn of an agent often says more than one message: an answer with tool calls, another after
their results, the final answer. Each message carries the turn it was said in (`turn_id`, issue
235), and so do the notices that end a turn or belong to it - a turn given up, a command carried
out, an activity. The web UI uses this only for the steps of a plan (`chatRows`, `PlanSteps`,
issue [263](263-plan-steps-not-in-chat.md)). Everywhere else every message is a bubble of its own,
headed with its sender's name, at the same distance from the next as any other.

So three messages of one turn look exactly like three turns of the same agent, and where agents
answer at once, the messages of their turns interleave without anything showing which belongs to
which.

## Why it matters

What a turn did is one unit: the reason for a tool call, its result and the answer drawn from it
belong together, and an answer is retried, stopped or given up as a turn. A reader who cannot see
where a turn begins and ends cannot tell what the agent answered to which message, nor which
answer a retry would replace.

## Considerations

The turn is known for every agent's message, so this is a question of the web UI alone. Four ways
to show it, which can be combined:

**A. Bubbles clustered as in messengers.** Consecutive messages of the same turn are drawn as one
cluster: the sender's name only above the first, a small gap between them and the usual one
between turns, the corner pointing at the sender only on the last bubble, time and actions only in
the last one's footer. Familiar from every messenger, costs no space and works in a narrow window.
Two turns of the same agent in a row are told apart by the name repeated and the larger gap.
Interleaved turns break a cluster in two, the second headed with the name again - which is what
happened.

**B. A rail beside the turn.** A thin vertical line, perhaps coloured per agent, runs along every
message of the turn, headed by a line such as "Helga · 14:02 · 3 messages · 2 tool calls". The same
form as the steps of a plan, and unambiguous even where turns interleave. Inside a plan it nests a
second line into the step's, which becomes restless; the step would have to give its line up and
keep only its heading.

**C. Folding what led to the answer.** The turn's last answer stays visible, everything before it -
intermediate answers, tool calls and their results - is folded under "Helga ran 4 steps", as the
steps of a plan are. The strongest change: it changes what is seen, not only how it is grouped, and
hides tool calls that wait for nothing but may still matter to the reader.

**D. Highlighting the turn on hover.** Pointing at a message gives every message of its turn a
faint background. Cheap, changes nothing at rest, and helps most where turns interleave. A
complement to any of the others, not a way of its own.

Where the grouping is computed: `chatRows` (`messages.ts`) already groups the messages into rows by
turn and step; the clustering belongs there, not in a second pass in `ChatView`. The answer under
way is rendered below every row and belongs to the last cluster where it is of that turn. Users'
messages have no turn and stay single.

## Decision

A and D first (2026-10-17); B and C are kept in reserve should A not suffice, C in particular for
long turns that take too much room.

## Done when

The messages of a turn - and the notices of it - are recognisably one group in the chat, live and
after a reload, two turns of the same agent in a row are told apart, and pointing at a message shows
which others belong to its turn.

## History

**2026-10-17:** Opened, with options A to D; A and D chosen to be built first.

**2026-10-17:** A and D built on the branch `issue-415-turn-grouping`: `turnGroups` (`messages.ts`)
groups the rows of `chatRows` by turn, `MessageView` takes where a message stands in its group
(`inTurn`), and `ChatView` shades the groups of the turn pointed at. To be tried in use before it is
done.

**2026-10-17:** Adjusted after a first look in use: the footers of every message are shown always -
hidden, they took their room all the same, and none showed while the turn went on -, and a turn of
more than one message is indented as a whole, its frame on hover flush with the messages alone and
as far from its messages as they are from their content.

**2026-10-17:** Done: the maintainer accepted A and D as adjusted; B and C stay in reserve.
