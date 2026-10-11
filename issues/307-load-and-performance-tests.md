# 307 — Nothing measures how Caelumora behaves under load

**Status:** open
**Area:** Observability and operations

## The problem

Caelumora has unit tests, store and security suites, but nothing that puts it under load. How many
users, sessions, channels and agents one instance carries, where the time of a request or a turn
goes, which locks, stores or streams give out first - nobody knows. Issues that touch it -
[21](21-scaling.md) (which scaling axis matters), [122](122-several-instances.md) (several
instances), [292](292-store-lists-read-everything.md) (lists read every entity) - argue from
reasoning, not from measurements.

A load test with real models is not possible: a model answers in seconds, costs money or a GPU,
and dominates every measurement, so that Caelumora's own cost disappears behind it.

Wanted, as the maintainer put it: a load test suite as an application of its own that drives the
REST server - simulated users that make users, sessions and channels at random and talk in them,
with the agents; simulated language models that answer far faster than real ones; load profiles as
configuration files. The aim is runtime profiles of Caelumora, to find its bottlenecks and remove
them. Written in Rust, or in Python if that is simpler.

## Why it matters

- **Optimising without measuring** guesses at the wrong place. A profile under realistic load
  shows where the time goes: the JSON store, the event bus, the fan-out of server-sent events,
  building a turn's context, the security checks.
- **Regressions go unnoticed.** A change that makes every list read every entity, or every turn
  re-read the whole history, works in a test with three channels and fails with three thousand.
- **The REST API gets a second client** besides the web UI, which keeps it honest: what a load test
  cannot do through the API, a user cannot either.

## Considerations

### Rust or Python

**Rust** (recommended):

- A member of the workspace, built, linted (`cargo clippy --workspace --all-targets` in the
  pre-commit hook) and refactored with the rest; a change of the REST API that breaks it is seen
  at once.
- tokio holds thousands of open event streams and connections cheaply - the streams are a likely
  bottleneck and have to be simulated in their thousands, as browsers hold them.
- The simulated model (below) is an HTTP server in the same process, as `FakeOllama` in
  `ollama_client.rs`'s tests already is.
- Histograms of latencies (`hdrhistogram`) and a report in JSON are little code.

**Python** (Locust) brings ramp-up, distribution over several machines, a web UI with live
statistics and reports ready-made. But Locust is made for request and response; long-lived
server-sent event streams, waiting for an agent's answer on them and serving a fake model besides
fit it badly, and the suite would live outside the workspace's build and lint.

**Goose**, a Locust-like framework for Rust, was considered: its model is the same request loop,
and its scenarios do not hold a stream across requests. A small scheduler of our own on tokio is
recommended; Goose can be reconsidered if distribution over several machines becomes necessary.

The REST types live in `caelumora-rest-server`; `caelumora-rest-client` is an empty crate. The suite
could make that crate a real client of the REST API - its requests and the types it reads - and be
its first user. Recommended, so that the suite does not depend on the server's internals and the
client can serve others later (the TUI speaks to the runtime directly).

### The simulated model

- **A. An `LLMClient` component of the runtime** (`mock_llm_client`): answers in-process, without
  HTTP. The fastest, deterministic. But test code in the production runtime, configurable in every
  deployment, and the model client's own work - HTTP, parsing the stream, building partial
  messages - is left out of the profile.
- **B. A fake Ollama server in the suite** (recommended): it serves the part of Ollama's API that
  `ollama_client` uses (`/api/chat` streamed, `/api/show`, `/api/ps`), and the personalities of the
  server under test are configured as usual, with their `endpoint` pointed at it. The real client
  path is profiled, nothing for tests enters the runtime, and the fake can run on another machine.
  It needs to follow Ollama's protocol as far as the client uses it, which `FakeOllama` shows is
  small.

What the fake answers, configurable per profile:

- **Latency** to the first chunk, **speed** in chunks per second, both drawn from a distribution;
  zero for a pure measure of Caelumora's own cost.
- **What it says**: an echo of the last message it was given, or filler text, of a length drawn
  from a distribution; optionally reasoning. Deterministic from the profile's seed.
- **Tool calls**, with a probability: a call of one of the tools it was offered, from a list of
  tools and arguments the profile names (`note`, `create_plan`, …) - so that tool execution,
  approvals and results are under load too. Calls that need approval have to be decided by the
  simulated users.
- **Token counts** in its answer, so that what depends on them (issue 209, compaction) runs.
- **Failures**, with a probability: an error, a broken stream - to see that a turn's error ends the
  turn under load as well.

### The simulated users

Each one an independent task, its actions drawn by weights the profile gives, with think times
between them:

- **Logging in** under a new random name: the server under test runs with the `accept_any` user
  provider, which makes a user on their first login (but see issue
  [306](306-user-records-have-no-writer.md)).
- **Making** sessions and channels, adding agents of the personalities the server's seed configures,
  inviting other simulated users.
- **Talking**: messages in a channel, some mentioning an agent; reading the history; deciding the
  approvals they are asked for; now and then modifying or taking back a message, issuing a command
  (`/compact`), opening the context of a message.
- **Listening**: holding the global stream and the streams of the channels they have open, as the
  web UI does, and waiting on them for the answers their messages asked for.

What a user waited for is checked: an agent mentioned answers, an event announced arrives. A lost
answer is counted as an error, not ignored - the suite finds what breaks under load, not only what
is slow.

### Load profiles

YAML files in the suite's directory (`profiles/`), each a scenario: its duration and ramp-up; how
many users arrive and how fast; how many sessions, channels and agents they make; the weights of
their actions and their think times; the fake model's latency, speed, length and tool calls; a seed.
Proposed to start with:

- **smoke** - a handful of users for a minute; also usable as a quick check that the suite and the
  API still fit.
- **steady** - a constant number of active users over a long time.
- **ramp** - users added until latencies or errors cross a limit: where does it give out.
- **idle-streams** - many users who mostly listen: the cost of open streams.
- **long-channels** - few channels with very long histories: what grows with history.
- **zero-latency model** - the model answers at once: Caelumora's own cost per turn.

With each profile goes the configuration and the seed of the server under test - users by
`accept_any`, personalities pointed at the fake model, a fresh data directory - so that a run can be
repeated. Its data is deleted afterwards.

### What is measured, and by whom

- **The suite** measures from outside: latency per endpoint and status, time from a message to the
  agent's first event and to its answer, events per second on the streams, errors and lost
  answers; as a summary on the console and a report in JSON, so that two runs can be compared.
- **The profile of the server** is taken by the usual tools, not by the suite: `perf` /
  `samply` / `cargo flamegraph` for the CPU, `tokio-console` for tasks and waits, `tracing` spans
  with timings. What is needed for that - a build profile with optimisation and debug symbols, a
  feature for `console-subscriber`, perhaps a layer that writes spans as a Chrome trace - is part
  of this issue; how to take and read a profile is described in a document of its own.
- Server-side metrics (counters, a Prometheus endpoint) would make runs easier to watch, but are an
  issue of their own if wanted.

### Not part of this issue

The optimisations themselves. Each bottleneck found becomes an issue of its own, with the profile
and the numbers that show it.

## Done when

- A binary in the workspace (`caelumora-loadtest`) runs a load profile against a running server:
  it makes users, sessions, channels and agents, talks in the channels, listens on the streams and
  decides approvals, and reports latencies, throughput, errors and lost answers.
- It serves a fake language model that the server's personalities are pointed at, with latency,
  speed, length, tool calls and failures configurable.
- The profiles above, each with the server configuration it runs against, are in the repository.
- A document describes how to run a profile and how to take and read a runtime profile of the
  server.

## History

- **2026-10-11** — Opened at the maintainer's request: a load test suite as an application of its
  own, with simulated users and fast simulated language models, load profiles as configuration
  files, to take runtime profiles and find bottlenecks. Recommended above: Rust, with a scheduler of
  its own on tokio; `caelumora-rest-client` made into a real client and used by the suite; the
  model faked as an Ollama server in the suite (B), not as a component of the runtime. Not decided
  yet.
