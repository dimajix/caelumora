# 195 — An answer that broke off can be said twice

**Status:** done
**Area:** Integrations

## The problem

When the stream of a request to an external agent (issue
[179](179-external-agents-via-a2a.md)) ends or breaks before its task does, the request is answered
with what came so far - `exchange` breaks out of its loop and `respond` publishes it as
`FullResponseGenerated`, or a stream error publishes `TurnFailed`. The task stays open (`Working`),
and `follow_open_tasks`, called right after the answer, follows it again. When it ends, `speak`
says what came of it on the agent's own account.

What the remote agent sends after it is re-attached depends on the remote side: if `resume` gives
the task's whole result (A2A's `GetTask`/`SubscribeToTask` answer with the task as it stands, its
artifacts included), the text that was already published cut off is said again, in full. The
channel then holds the answer twice, once cut off, and so does the agent's history - the remote
agent is not given its own answers, so this does not loop back, but every other participant hears
both.

The followed path no longer does this since issue [184](184-followed-task-ends-said-wrongly.md):
there a stream that ends early says nothing and leaves the task to be followed again. The request
path cannot simply do the same - a request is always answered (`respond`: "A request is always
answered, with nothing if need be").

## Why it matters

Whoever waits for the answer sees a fragment as the agent's answer, and later the whole answer as
a second message, with nothing that relates the two.

## Considerations

- **Answer the request with nothing, or with a note, and leave the text to the followed task.** The
  request ends as answered, without a message or with a short one ("still working - the answer
  follows"), and the task is said once, in full, when it ends. Loses what was shown as partials
  if the remote agent does not repeat it on `resume`.
- **Remember what was published for the task** (e.g. the published text, or its length, beside the
  `OpenTask`) and say only what came after it. Depends on `resume` giving the whole result, which
  is the remote side's choice; a remote agent that only sends what is new would lose nothing either
  way, but the two cannot be told apart from the events.
- **Say the end as a correction of the answer** - the message that was published cut off is
  replaced by the whole one. The channel has no notion of a message replacing another, so this
  would be a new event and a change to every frontend.
- What A2A 1.0 promises about a re-attached task's artifacts should be checked against the
  specification and a real server first (as for issue [186](186-a2a-requests-and-long-tasks.md));
  the first option is independent of it.

## Done when

- An answer that broke off before its task ended is not said a second time, in full, when the task
  ends; covered by a test with a remote agent that repeats its result on `resume`.

## History

- **2026-10-02** — Opened while fixing issue 184, which removed the same duplication from the
  followed path; the request path answers what came so far on purpose, and still leaves the task to
  be said again.
- **2026-10-02** — Done, as the maintainer decided: the second option, together with the first. A
  request whose stream ends or fails before its task does is answered with nothing (an empty
  `FullResponseGenerated`, which tells observers the answer ended) instead of with the fragment,
  and a stream error no longer reports `TurnFailed` - the task has not failed, it goes on. What came
  so far is kept with the open task (`OpenTask::unsaid`, stored) and the followed task's answer
  begins with it (`Answer::after`): when the task ends, its result is said alone if it begins with
  what was kept - the remote agent repeated it - and after it otherwise. Either way it is said once,
  as one message. A question back clears what was kept, since it was said with the question. A
  stream without a task still answers with what came, or fails. The test of issue 184 that expected
  `TurnFailed` for a broken stream expects the empty answer now; a new test covers a remote agent
  that repeats its result on `resume` and one that only goes on.
