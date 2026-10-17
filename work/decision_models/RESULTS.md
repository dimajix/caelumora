# Decision models choosing who responds

Issue 403. `probe.py` asks each model which of six agents - coder, tester, reviewer, ops,
researcher, planner, each with a one-line introduction - should respond to 16 messages, in English
and German, three of which need no answer; the question is the `choice` that `decision_policy`
builds. Measured on 2026-10-17 against Ollama 0.40.6 on the maintainer's machine, one run each,
after one call to load the model.

| Model | Right | p(right) mean | p(right) min | Median latency |
|---|---:|---:|---:|---:|
| tev1:0.8b | 11/16 | 0.59 | 0.04 | 83 ms |
| laya:421m-typed-decisions-bf16 | 8/16 | 0.30 | 0.04 | 90 ms |
| d1:3b | 15/16 | 0.72 | 0.25 | 151 ms |
| tev1:4b | 15/16 | 0.79 | 0.39 | 268 ms |
| clef-flash:9b | 16/16 | 0.86 | 0.62 | 298 ms |
| nimble:9b | 16/16 | 0.95 | 0.79 | 327 ms |
| pplx-decider:27b | 16/16 | 0.95 | 0.86 | 690 ms |

An easier first probe - three agents, twelve messages - did not tell the models apart: tev1:0.8b,
d1:3b, tev1:4b, nimble:9b, clef-flash:9b and pplx-decider:27b got all right;
laya:322m-multilingual-bf16 got 6, laya:421m-english-bf16 9, laya:421m-typed-decisions-bf16 11.

The small models mostly fail by choosing nobody where someone should answer. nimble:9b is as right
and as sure as the 27B model at half its latency. Loading nimble:9b took 81 s once, with other
models in memory: far beyond the 5 s a decision may take by default, so it is worth keeping loaded
(`keep_alive`).

Few cases, one run: enough to tell a model that works from one that does not, not to rank close
ones, and nothing about which `threshold` suits a model.
