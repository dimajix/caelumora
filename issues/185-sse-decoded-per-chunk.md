# 185 — The A2A event stream is decoded chunk by chunk

**Status:** done
**Area:** Integrations

## The problem

`events_of` in `external_impl/a2a_agent.rs` parses the server-sent events of a streamed A2A request
by hand. It decodes every network chunk on its own - `String::from_utf8_lossy(&chunk)` and
`.replace("\r\n", "\n")` - before looking for the blank line that ends an event:

- a character of several bytes split between two chunks (an "ä") becomes U+FFFD;
- a `\r\n` split between two chunks leaves a `\r`, the end of the event is not found, two events run
  together, their JSON does not parse and the stream ends with `InvalidResponse`;
- the buffer is searched from the start after every chunk, quadratic for a large event that arrives
  in many chunks.

## Why it matters

Whether an answer arrives intact depends on where the network happens to split it - a failure that
shows rarely and cannot be reproduced on purpose.

## Considerations

- **`sse-stream`**, already compiled into the workspace through `rmcp`: the event framing is done by
  a library, and only the JSON-RPC decoding and `Translation` stay here.
- **A byte buffer**: split on the blank line in bytes, decode each complete event once, and search
  only the new part of the buffer.
- `ollama_client.rs` decodes its stream chunk by chunk too (`from_utf8_lossy`), from before issue
  179; it is worth checking with the same fix.

## Done when

- Events split anywhere - in a character, in a line end - are read intact, covered by a test that
  feeds the stream in single bytes.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series; confirmed in
  `events_of`.
- **2026-10-02** — Done. `events_of` frames the events with `sse-stream`, now a direct dependency,
  and decodes each one whole; only the JSON-RPC decoding and `Translation` stay. `ollama_client.rs`
  had the same fault worse - a JSON line split by the network was dropped with a warning, its
  tokens lost - and now reads its lines through `tokio-util`'s `LinesCodec` in `answer_of`. Both
  are covered by a test that feeds the stream in single bytes.
