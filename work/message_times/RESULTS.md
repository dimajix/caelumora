# Times in the model input (issue 424) - experiments of 2026-10-18

gemma4:26b (Q4_K_M) on Ollama, temperature 0.7, 5 runs per probe, 1-2 runs of the twelve-round
chat, English chats. `experiment.py` sends what Caelumora sends today - the roster part with the
sender explanation, `<message from="@handle">` or `@handle: ...` - extended by when a message was
written: `<message from="@alice" at="Sun 18 Oct 2026, 14:32">` or `@alice (Sun 18 Oct 2026,
14:32): ...`, with the explanation saying so. `run_all.sh` holds the runs, `rescore.py` scores
the answers again with corrected checks; raw answers in `results/*.jsonl` (not committed). Scoring
is by keywords; the tables below are after reading the answers and correcting the checks.

## Findings

1. **The time on the others' messages is used.** Without it the model answers "tomorrow at
   10:00" for a review announced three days earlier, and does not know on which day something was
   proposed; with it, it names the date (`probes`):

   | time on the others' messages | review already past | migration done | which day | no time asked |
   |---|---:|---:|---:|---:|
   | none | 0/5 | 2/5 (only "if 40 minutes have passed") | 0/5 | 5/5 |
   | `2026-10-18 14:32` | 5/5 | 5/5 | 4/5 | 5/5 |
   | `Sun 18 Oct 2026, 14:32` | 5/5 | 5/5 | 5/5 | 5/5 |

   It names the date of the review rather than saying that it is over. The weekday is surer with
   the long form, which names it; the numeric form makes the model work it out.
2. **The agent's own messages must carry no time.** With `[Sun 18 Oct 2026, 14:32] ...` on its own
   messages, the model begins its answers with a time of its own - a plausible one, made up - in
   25 of 48 answers with XML and 8 of 24 with the prefix format (`imitation`). With times on the
   others' messages only, it never did: 0 of 72, in both formats. No answer to a question without
   time carried one (0 of 60 in `probes`). As with the sender (`work/sender_names/RESULTS.md`,
   finding 1), the explanation does not stop it.
3. **The model takes the time of the last message for now.** With times on the messages and no
   current time, it knew the date, the weekday and that the bakery is closed on Sundays - from the
   last message, which was said a minute before (`now`, every position 5/5, also none).
4. **That is wrong as soon as the turn comes later than the last message.** A question asked on
   Friday at 17:55 and answered on Sunday - a late turn, after `/wakeup`, a long queue: without the
   current time "Yes, you have about 5 minutes left before they close" in 5 of 5 answers; with it,
   "No, the bakery is closed on Sundays" in 5 of 5, at every position (`now-late`).
5. **Without times on the messages, only the current time tells the date.** Without either, the
   model says "Today is May 22, 2024"; with the current time it knows the date, the weekday and
   the day after tomorrow, at every position (`now-untimed`). How long ago something was said it
   cannot know without the times on the messages (build done: 0-1/5).
6. **The position of the current time makes no difference to gemma4**: in the system prompt,
   before the newest message (`BeforeTurnInput`) or after it at the very end - the same answers
   (findings 3-5). It does not distract either: a translation asked for came without date or time
   at every position (25 of 25); at the end it once began "Bonjour @alice," before the
   translation.

7. **In a German channel every form works, and none pulls the model into English** (`probes-de`:
   the same probes in German, the channel's language told as `channel_language_prompt_provider`
   tells it):

   | time on the others' messages | review already past | migration done | which day | no time asked | answers in German |
   |---|---:|---:|---:|---:|---:|
   | none | 0/5 | 0/5 | 0/5 | 5/5 | 20/20 |
   | `2026-10-18 14:32` | 5/5 | 5/5 | 5/5 | 5/5 | 20/20 |
   | `Sun 18 Oct 2026, 14:32` | 5/5 | 5/5 | 5/5 | 5/5 | 20/20 |
   | `So. 18. Okt. 2026, 14:32` | 5/5 | 5/5 | 5/5 | 5/5 | 20/20 |

   No answer carried a time of its own (0 of 80). With either long form the model names the
   weekday ("am Mittwoch, den 14. Oktober"), with the numeric one only the date. The English form
   in a German chat did no harm: the answers name the day in German. (The column "German" in
   `results/probes-de.log` is wrong: its first check counted German words and missed most German
   answers; read again, all 80 are German.)

## Not measured

- Other languages than English and German.
- Other models, and gemma4 over longer conversations with a current time in every call.
- Whether the model mentions on its own that a long time has passed ("welcome back") - after the
  gap of two days in the twelve-round chat it never did, with times or without.
