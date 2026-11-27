# 625 — A model's control tokens reach the channel as its answer

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Integrations
**Summary:** What an Ollama model produces is passed on as it comes, so a control token the model writes as text - one of gemma4 - becomes, or becomes part of, the agent's answer in the channel.

## The problem

In the experiment of issue [616](616-a-model-misreads-what-starting-a-sub-agent-does.md)
(`work/subagent_start`), `gemma4:26b` on Ollama ended a turn with an answer whose whole content was
`<|tool_response>`, no call beside it; its thinking said it meant to wait for the next turn. Of 80
runs, 14 had such an answer from the assistant or its sub-agent: `<|tool_response>` 13 times,
`<channel|>` once, mid-text before a call. Both are control tokens of gemma4, as `/api/show` lists
them (type 3 among `tokenizer.ggml.token_type`; gemma4 has 24, among them `<|tool_call>`,
`<|tool_response>`, `<|channel>` and their closing ones). Ollama's parser for the model does not
take them out of `content`.

The Ollama client (`caelumora-model-ollama/src/client.rs`) passes what the model produces through
as it comes: each piece streamed as a partial message, and the whole as the message the turn
records. So the token is shown as the agent's answer, recorded in the channel's history, and given
back to the model in later turns - defused then, by a zero-width space (issue
[149](149-special-tokens-in-content.md)): the transcript of the experiment holds an assistant
message `"\n<​channel|>"`.

## Why it matters

- The person sees an answer that is no answer: a token of the model's format, where nothing - or
  the end of a turn - was meant.
- It is kept: in the history, in the transcript, in what the model is given again.

## Considerations

- **Where it belongs.** In the Ollama client, as the defusing of what goes in (issue 149): which
  strings are control tokens is a property of the backend's tokenizer, and the client reads them
  once per model already (`ControlTokens`). A control token in what a model produces is never meant
  as text.
- **Taken out or defused.** Taken out: defused, the token would still be shown, only no longer a
  token. Logged as it is taken out, so that a model that leaks stays visible.
- **The partials.** What is streamed is shown as it comes: the token has to be taken out of each
  partial as well as of the whole. Ollama streams a token in one piece, so one partial holds a
  control token whole; whether a piece can end inside one is to be checked.
- **What is left.** An answer of control tokens alone becomes empty - to be checked that the agent
  takes an empty answer as one that says nothing, as it takes one Ollama returns empty.
- **The thinking.** It is shown to whoever may see it; the same holds for it.
- **Stop sequences** instead - the tokens given to Ollama as `stop` - would end the answer at the
  first one, which fits `<|tool_response>` but not a token mid-text, and is a list per model the
  client would keep beside the one it reads.
- **Upstream.** The parser of Ollama for gemma4 could take the tokens out itself; whether to report
  it there is the maintainer's call. The client's own handling is needed either way: another model,
  or another version, can leak another token.

## Done when

- No control token of the model is in what the Ollama client gives of an answer, streamed or whole;
  what was taken out is logged; covered by tests of the client.
- An answer of control tokens alone ends the turn as an empty one; covered by a test.

## History

- **2026-11-27** — Opened on what the experiment of issue 616 recorded.
