#!/usr/bin/env python3
"""Times in the model input (issue 424) - experiments against a local Ollama.

Three experiments, each a subcommand, with the prompts Caelumora sends today
(the roster part with the sender explanation, `<message from="@handle">` or
`@handle: ...`), extended by when a message was written:

  imitation  A twelve-round chat in which the model writes the agent's answers
             itself (as `work/sender_names/simulate.py`): does it start to
             write times - or markers - of its own, when the others' messages
             carry a time, and when its own carry one too?
  probes     Single answers to fixed chats whose right answer needs the times
             of the messages: a review that already took place, a migration
             that should be done, the day something was proposed.
  now        The current time as a prompt part, at four positions: is the
             date known, is it used where it matters, and does it distract
             where it does not?

Every request and answer goes to results/<experiment>.jsonl; a table is
printed at the end. Scoring is by keywords and meant as a first sieve - the
answers are kept for reading.

Usage:
    python3 experiment.py imitation --runs 2
    python3 experiment.py probes --runs 5
    python3 experiment.py now --runs 5
"""

import argparse
import collections
import datetime as dt
import json
import pathlib
import re
import urllib.request

OLLAMA = "http://127.0.0.1:11434/api/chat"
RESULTS = pathlib.Path(__file__).parent / "results"
AGENT = "Nova"

# The proxy of the machine refuses localhost: no proxy for Ollama.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def chat(model, messages, temperature):
    body = {"model": model, "messages": messages, "stream": False,
            "options": {"temperature": temperature, "num_ctx": 16384}}
    req = urllib.request.Request(OLLAMA, json.dumps(body).encode(), {"Content-Type": "application/json"})
    with OPENER.open(req, timeout=900) as resp:
        data = json.load(resp)
    return data["message"].get("content", "")


# --- How a time is written --------------------------------------------------

DAYS_EN = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
MONTHS_EN = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
DAYS_DE = ["Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa.", "So."]
MONTHS_DE = ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sep.", "Okt.", "Nov.", "Dez."]

STYLES = {
    "none": None,
    # Numeric, the same in every language.
    "iso": lambda t: t.strftime("%Y-%m-%d %H:%M"),
    # Weekday and month by name, in English and in German.
    "long_en": lambda t: f"{DAYS_EN[t.weekday()]} {t.day} {MONTHS_EN[t.month - 1]} {t.year}, {t:%H:%M}",
    "long_de": lambda t: f"{DAYS_DE[t.weekday()]} {t.day}. {MONTHS_DE[t.month - 1]} {t.year}, {t:%H:%M}",
}
EXAMPLE = dt.datetime(2026, 10, 18, 14, 32)


def stamp(style, t):
    return STYLES[style](t) if STYLES[style] else None


def other(fmt, style, sender, text, t):
    """A message of another participant, marked as Caelumora marks it, with its time."""
    at = stamp(style, t)
    if fmt == "xml":
        return f'<message from="@{sender}" at="{at}">{text}</message>' if at else f'<message from="@{sender}">{text}</message>'
    return f"@{sender} ({at}): {text}" if at else f"@{sender}: {text}"


def own(own_style, style, text, t):
    """One of the agent's own messages: unmarked, or with its time in brackets."""
    at = stamp(style, t)
    return f"[{at}] {text}" if own_style == "stamped" and at else text


def explanation(fmt, style, own_style):
    """`ModelProfile::sender_explanation`, saying when a message was written where it says so."""
    example = stamp(style, EXAMPLE)
    if fmt == "xml":
        marked = (f'are wrapped in an element naming their sender\'s @handle and when it was written: '
                  f'<message from="@handle" at="{example}">...</message>' if example else
                  'are wrapped in an element naming their sender\'s @handle: <message from="@handle">...</message>')
    else:
        marked = (f'begin with their sender\'s @handle, when it was written and a colon: "@handle ({example}): ..."'
                  if example else 'begin with their sender\'s @handle and a colon: "@handle: ..."')
    if own_style == "stamped" and example:
        mine = (f"Your own messages begin with when they were written, \"[{example}] ...\": that is added for you - "
                "answer without it, and never write a message for someone else.")
    else:
        mine = "Your own messages carry no such marker: answer without one, and never write a message for someone else."
    return f"Messages of the others {marked}. {mine}"


def system(fmt, style, own_style, others, extra=None):
    """The system prompt: a personality's own, the roster part and any other part at the start."""
    roster = "\n".join(f"- {name.capitalize()} (@{name}), {kind}." for name, kind in others)
    parts = [
        "You are a helpful assistant in a team chat. Be brief.",
        f"[Who is in this channel]\nYou are {AGENT} (@nova), an agent.\n"
        f"The others - mention one by their @handle to address them, or all of them with @all:\n{roster}\n"
        f"{explanation(fmt, style, own_style)}\n",
    ]
    if extra:
        parts.append(extra)
    return "\n\n".join(parts)


# --- Detecting imitation ------------------------------------------------------

TIME_START = re.compile(
    r"^\W{0,4}(\d{4}-\d{2}-\d{2}|\d{1,2}:\d{2}|\d{1,2}\.\s?\d{1,2}\.|"
    r"(Mon|Tue|Wed|Thu|Fri|Sat|Sun|Mo|Di|Mi|Do|Fr|Sa|So)\.?\s+\d)", re.IGNORECASE)


def imitates(answer):
    """Whether `answer` begins with a time or a marker of its own, or writes one anywhere."""
    text = answer.strip()
    return bool(TIME_START.match(text) or re.match(r"^\W{0,4}(<message|@nova\b)", text, re.IGNORECASE)
                or re.search(r'\bat="', text))


def voices(answer, names):
    """Lines in which the model writes for someone else."""
    lines = answer.splitlines()[1:]
    return [line.strip()[:70] for line in lines for n in names
            if re.match(rf"^\W{{0,4}}(<message from=\"@{n}\"|@{n}\b\s*(\([^)]*\))?\s*:)", line.strip(), re.IGNORECASE)]


# --- Experiment: imitation ------------------------------------------------------

ROUNDS = [
    (0, [("alice", "Hi Nova! We need to plan the Q3 hackathon. Can you propose a two-day agenda?")]),
    (6, [("bob", "Looks good, but day 1 starts too early. Most of us commute."),
         ("carol", "Agreed with Bob. Also, we need a slot for the security talk.")]),
    (15, [("alice", "@nova please update the agenda with Bob's and Carol's points.")]),
    (40, [("atlas", "I'm Atlas, the facilities agent. Room 4.12 is free both days; the auditorium only on day 2."),
          ("dave", "Then the kick-off should be in the auditorium on day 2? That feels backwards.")]),
    (44, [("carol", "@nova what do you think about Dave's concern?")]),
    (90, [("bob", "Can someone summarize who is organizing what so far? I lost track.")]),
    # Two days later.
    (2 * 24 * 60 + 30, [("alice", "Morning! I'll do catering. Carol does the security talk. Dave, can you take the prizes?"),
                        ("dave", "Sure, I'll handle the prizes."),
                        ("erin", "Hi, I'm new. @nova what did I miss? Keep it short.")]),
    (2 * 24 * 60 + 45, [("atlas", "Correction: the auditorium is now also free on day 1 afternoon."),
                        ("bob", "@nova does that change your agenda?")]),
    (2 * 24 * 60 + 50, [("carol", "The security talk needs 45 minutes, not 30. Can you fix that?")]),
    (2 * 24 * 60 + 200, [("erin", "I could run a workshop on the new API. Where would it fit?")]),
    (3 * 24 * 60 + 10, [("alice", "@nova please post the final agenda, with who owns which slot.")]),
    (3 * 24 * 60 + 15, [("dave", "@nova who asked for the security slot originally, and when?")]),
]
START = dt.datetime(2026, 10, 12, 9, 5)
ROSTER = [("alice", "a person"), ("bob", "a person"), ("carol", "a person"), ("dave", "a person"),
          ("erin", "a person"), ("atlas", "an agent")]


def run_imitation(args, out):
    rows = []
    for fmt in args.formats:
        for style in args.styles:
            for own_style in (["plain"] if style == "none" else args.own):
                for run in range(args.runs):
                    messages = [{"role": "system", "content": system(fmt, style, own_style, ROSTER)}]
                    tagged = voiced = 0
                    for i, (minutes, said) in enumerate(ROUNDS, 1):
                        at = START + dt.timedelta(minutes=minutes)
                        for k, (sender, text) in enumerate(said):
                            messages.append({"role": "user", "content": other(fmt, style, sender, text, at + dt.timedelta(minutes=k))})
                        answer = chat(args.model, messages, args.temperature)
                        tag, lines = imitates(answer), voices(answer, [n for n, _ in ROSTER])
                        tagged += tag
                        voiced += bool(lines)
                        out.write(json.dumps({"experiment": "imitation", "format": fmt, "style": style, "own": own_style,
                                              "run": run, "round": i, "imitates": tag, "voices": lines, "answer": answer}) + "\n")
                        out.flush()
                        if args.show:
                            print(f"--- {fmt} {style} own={own_style} run {run} round {i} imitates={int(tag)} voices={lines}\n{answer.strip()[:600]}")
                        # Stored as a runtime stores it: as written, with its time where its own carry one.
                        messages.append({"role": "assistant", "content": own(own_style, style, answer.strip(), at + dt.timedelta(minutes=len(said) + 1))})
                    rows.append((fmt, style, own_style, tagged, voiced, len(ROUNDS)))
                    print(f"imitation {fmt:6} {style:8} own={own_style:8} run {run}: imitates {tagged}/{len(ROUNDS)}, voices {voiced}/{len(ROUNDS)}", flush=True)
    table = collections.defaultdict(lambda: [0, 0, 0])
    for fmt, style, own_style, tagged, voiced, n in rows:
        t = table[(fmt, style, own_style)]
        t[0] += tagged
        t[1] += voiced
        t[2] += n
    print("\n| format | style | own messages | imitates | writes for others |\n|---|---|---|---:|---:|")
    for (fmt, style, own_style), (tagged, voiced, n) in table.items():
        print(f"| {fmt} | {style} | {own_style} | {tagged}/{n} | {voiced}/{n} |")


# --- Experiment: probes ---------------------------------------------------------

def d(day, hour, minute):
    return dt.datetime(2026, 10, day, hour, minute)


# Each probe: the conversation (sender, text, time; sender None for Nova's own),
# and a check of the answer.
PROBES = {
    # The review was on Tuesday at 10:00; on Thursday it is past.
    "past_review": ([
        ("alice", "@nova The release review is tomorrow at 10:00. What should we prepare?", d(12, 9, 5)),
        (None, "Prepare the changelog, the test report and the list of open issues.", d(12, 9, 6)),
        ("bob", "@nova When is the release review? I want to put it into my calendar.", d(15, 16, 40)),
    ], lambda a: bool(re.search(r"\b13(th)?\b|tuesday|already|has passed|in the past|took place|was on|was held|missed", a, re.I))),
    # 68 minutes after a migration of about 40 minutes: it should be done.
    "elapsed": ([
        ("carol", "@nova I started the database migration just now. It usually takes about 40 minutes.", d(16, 14, 2)),
        ("dave", "Coffee anyone?", d(16, 14, 5)),
        ("carol", "@nova Should the migration be finished by now?", d(16, 15, 10)),
    ], lambda a: bool(re.search(r"\byes\b|should (be|have) (finished|done|complete)|14:42|over an hour|68|more than (an hour|40)|already", a, re.I))
       and not re.search(r"(can't|cannot|don't|do not) (know|tell|determine)", a, re.I)),
    # Erin proposed the workshop on Wednesday, 14 October.
    "which_day": ([
        ("alice", "Let's collect ideas for the hackathon here.", d(12, 10, 0)),
        ("bob", "A talk on observability would be nice.", d(13, 11, 20)),
        ("erin", "I could run a workshop on the new API.", d(14, 15, 45)),
        ("carol", "And a security talk, 45 minutes.", d(15, 9, 10)),
        ("dave", "@nova On which day did Erin propose the API workshop?", d(16, 8, 30)),
    ], lambda a: bool(re.search(r"wednesday|\b14\b", a, re.I))),
    # Nothing about time is asked: an answer should carry no time of its own.
    "plain": ([
        ("alice", "@nova Can you give me three name ideas for our new build server?", d(16, 10, 0)),
    ], lambda a: not imitates(a)),
}
# The same probes in a German channel, which the agent is told as the
# `channel_language_prompt_provider` tells it.
LANGUAGE_DE = ('[Language of this channel]\nThis channel\'s conversation is held in German - Deutsch, BCP 47 tag "de". '
               'Write everything in German - your answers, your messages to the others and what you note down - '
               'whatever language your instructions, your tools or earlier messages are in.\n')
PROBES_DE = {
    "past_review": ([
        ("alice", "@nova Das Release-Review ist morgen um 10:00. Was sollen wir vorbereiten?", d(12, 9, 5)),
        (None, "Bereitet das Changelog, den Testbericht und die Liste offener Punkte vor.", d(12, 9, 6)),
        ("bob", "@nova Wann ist das Release-Review? Ich will es in meinen Kalender eintragen.", d(15, 16, 40)),
    ], lambda a: bool(re.search(r"\b13\.|dienstag|bereits|vorbei|stattgefunden|war am|verpasst", a, re.I))),
    "elapsed": ([
        ("carol", "@nova Ich habe gerade die Datenbankmigration gestartet. Die dauert normalerweise etwa 40 Minuten.", d(16, 14, 2)),
        ("dave", "Kaffee, jemand?", d(16, 14, 5)),
        ("carol", "@nova Müsste die Migration inzwischen fertig sein?", d(16, 15, 10)),
    ], lambda a: bool(re.search(r"\bja\b|sollte (sie )?(bereits |schon |längst )?(fertig|abgeschlossen|durch)|14:42|über eine stunde|68|bereits|längst", a, re.I))
       and not re.search(r"(kann|weiß) (ich )?(das )?nicht", a, re.I)),
    "which_day": ([
        ("alice", "Lasst uns hier Ideen für den Hackathon sammeln.", d(12, 10, 0)),
        ("bob", "Ein Vortrag über Observability wäre schön.", d(13, 11, 20)),
        ("erin", "Ich könnte einen Workshop zur neuen API machen.", d(14, 15, 45)),
        ("carol", "Und einen Security-Vortrag, 45 Minuten.", d(15, 9, 10)),
        ("dave", "@nova An welchem Tag hat Erin den API-Workshop vorgeschlagen?", d(16, 8, 30)),
    ], lambda a: bool(re.search(r"mittwoch|\b14\.", a, re.I))),
    "plain": ([
        ("alice", "@nova Kannst du mir drei Namensideen für unseren neuen Build-Server geben?", d(16, 10, 0)),
    ], lambda a: not imitates(a)),
}


def german(answer):
    """Whether `answer` stayed German: not two of the most common English words in it. (A list of
    German words, used first, missed most German answers.)"""
    return len(re.findall(r"\b(the|is|was|should|have|has|it|you|on|at|and|of|be|will|by)\b", answer, re.I)) < 2


PROBE_ROSTER = [("alice", "a person"), ("bob", "a person"), ("carol", "a person"), ("dave", "a person"), ("erin", "a person")]


def probe_messages(fmt, style, own_style, said, extra=None):
    messages = [{"role": "system", "content": system(fmt, style, own_style, PROBE_ROSTER, extra)}]
    for sender, text, at in said:
        if sender is None:
            messages.append({"role": "assistant", "content": own(own_style, style, text, at)})
        else:
            messages.append({"role": "user", "content": other(fmt, style, sender, text, at)})
    return messages


def run_probes(args, out):
    probes, extra = (PROBES_DE, LANGUAGE_DE) if args.lang == "de" else (PROBES, None)
    table = collections.defaultdict(lambda: [0, 0, 0, 0])
    for style in args.styles:
        for name in args.probes:
            said, check = probes[name]
            for run in range(args.runs):
                answer = chat(args.model, probe_messages(args.format, style, "plain", said, extra), args.temperature)
                right, tag, de = check(answer), imitates(answer), german(answer)
                t = table[(style, name)]
                t[0] += right
                t[1] += tag
                t[2] += 1
                t[3] += de
                out.write(json.dumps({"experiment": "probes", "lang": args.lang, "format": args.format, "style": style, "probe": name,
                                      "run": run, "right": right, "imitates": tag, "german": de, "answer": answer}) + "\n")
                out.flush()
                print(f"probes {style:8} {name:12} run {run}: right={int(right)} imitates={int(tag)} german={int(de)}  {answer.strip()[:140]!r}", flush=True)
    print(f"\n| style | " + " | ".join(args.probes) + " | imitates | German |\n|---|" + "---:|" * (len(args.probes) + 2))
    for style in args.styles:
        cells = [f"{table[(style, p)][0]}/{table[(style, p)][2]}" for p in args.probes]
        tagged = sum(table[(style, p)][1] for p in args.probes)
        de = sum(table[(style, p)][3] for p in args.probes)
        n = sum(table[(style, p)][2] for p in args.probes)
        print(f"| {style} | " + " | ".join(cells) + f" | {tagged}/{n} | {de}/{n} |")


# --- Experiment: the current time -------------------------------------------------

NOW = d(18, 14, 33)  # a Sunday


def now_part(style):
    if style == "long_de":
        text = f"Es ist {['Montag', 'Dienstag', 'Mittwoch', 'Donnerstag', 'Freitag', 'Samstag', 'Sonntag'][NOW.weekday()]}, " \
               f"18. Oktober 2026, 14:33 (Europe/Berlin)."
    else:
        text = f"It is {NOW:%A}, {NOW.day} {NOW:%B %Y}, {NOW:%H:%M} (Europe/Berlin)."
    return f"[Current date and time]\n{text}\n"


NOW_PROBES = {
    "date": ([
        ("alice", "@nova What's the date today?", d(18, 14, 32)),
    ], lambda a: bool(re.search(r"\b18\b", a) and re.search(r"oct", a, re.I))),
    # The bakery is closed on Sundays.
    "open_now": ([
        ("bob", "@nova The bakery downstairs is open Monday to Saturday, 7:00 to 18:00. Can I still get bread there now?", d(18, 14, 32)),
    ], lambda a: bool(re.search(r"\bno\b|closed|sunday|not open|won't be able|can't|cannot", a, re.I))),
    # The day after tomorrow is Tuesday, 20 October.
    "relative_date": ([
        ("carol", "@nova Please write a one-line reminder for our team meeting the day after tomorrow at 10:00, with its date.", d(18, 14, 32)),
    ], lambda a: bool(re.search(r"\b20\b|tuesday", a, re.I))),
    # 88 minutes after a build of about 30 minutes.
    "build_done": ([
        ("dave", "I started the build; it takes about 30 minutes.", d(18, 13, 5)),
        ("erin", "Thanks Dave.", d(18, 13, 6)),
        ("alice", "@nova Should Dave's build be done by now?", d(18, 14, 32)),
    ], lambda a: bool(re.search(r"\byes\b|should (be|have) (finished|done|complete)|13:35|over an hour|8[78]|already", a, re.I))
       and not re.search(r"(can't|cannot|don't|do not) (know|tell|determine)", a, re.I)),
    # Asked on Friday shortly before closing, answered on Sunday - a late
    # turn, after `/wakeup` or a long queue: it is closed now.
    "late_open_now": ([
        ("bob", "@nova The bakery downstairs is open Monday to Saturday, 7:00 to 18:00. Can I still get bread there now?", d(16, 17, 55)),
    ], lambda a: bool(re.search(r"\bno\b|closed|sunday|not open|won't be able|can't|cannot", a, re.I))),
    # Nothing about time is asked: the answer should not mention it.
    "distraction": ([
        ("alice", "@nova Translate 'Good morning, team - the deploy went fine' into French.", d(18, 14, 32)),
    ], lambda a: not re.search(r"14:3\d|sunday|october|\b18\b|current (date|time)|date and time|thanks? for the (time|update)", a, re.I)),
}
POSITIONS = ["none", "system", "before_input", "end"]


def run_now(args, out):
    table = collections.defaultdict(lambda: [0, 0, 0])
    for position in args.positions:
        for name in args.probes:
            said, check = NOW_PROBES[name]
            for run in range(args.runs):
                part = now_part(args.style)
                messages = probe_messages(args.format, args.message_style or args.style, "plain", said, part if position == "system" else None)
                if position == "before_input":
                    messages.insert(len(messages) - 1, {"role": "user", "content": part})
                elif position == "end":
                    messages.append({"role": "user", "content": part})
                answer = chat(args.model, messages, args.temperature)
                right, tag = check(answer), imitates(answer)
                t = table[(position, name)]
                t[0] += right
                t[1] += tag
                t[2] += 1
                out.write(json.dumps({"experiment": "now", "format": args.format, "style": args.style, "message_style": args.message_style or args.style, "position": position,
                                      "probe": name, "run": run, "right": right, "imitates": tag, "answer": answer}) + "\n")
                out.flush()
                print(f"now {position:12} {name:14} run {run}: right={int(right)} imitates={int(tag)}  {answer.strip()[:140]!r}", flush=True)
    print(f"\n| position | " + " | ".join(args.probes) + " | imitates |\n|---|" + "---:|" * (len(args.probes) + 1))
    for position in args.positions:
        cells = [f"{table[(position, p)][0]}/{table[(position, p)][2]}" for p in args.probes]
        tagged = sum(table[(position, p)][1] for p in args.probes)
        n = sum(table[(position, p)][2] for p in args.probes)
        print(f"| {position} | " + " | ".join(cells) + f" | {tagged}/{n} |")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("experiment", choices=["imitation", "probes", "now"])
    ap.add_argument("--model", default="gemma4:26b")
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("--temperature", type=float, default=0.7)
    ap.add_argument("--formats", nargs="+", default=["xml", "prefix"], help="imitation")
    ap.add_argument("--format", default="xml", help="probes, now")
    ap.add_argument("--styles", nargs="+", default=["none", "iso", "long_en"], help="imitation, probes")
    ap.add_argument("--style", default="long_en", help="now: of the part")
    ap.add_argument("--message-style", default=None, help="now: of the messages; that of the part if not given")
    ap.add_argument("--own", nargs="+", default=["plain", "stamped"], help="imitation")
    ap.add_argument("--probes", nargs="+", default=None)
    ap.add_argument("--lang", default="en", choices=["en", "de"], help="probes: the language of the chat")
    ap.add_argument("--positions", nargs="+", default=POSITIONS, help="now")
    ap.add_argument("--show", action="store_true")
    ap.add_argument("--tag", default="", help="suffix of the results file")
    args = ap.parse_args()
    RESULTS.mkdir(exist_ok=True)
    with open(RESULTS / f"{args.experiment}{args.tag}.jsonl", "a") as out:
        if args.experiment == "imitation":
            run_imitation(args, out)
        elif args.experiment == "probes":
            args.probes = args.probes or list(PROBES)
            run_probes(args, out)
        else:
            args.probes = args.probes or list(NOW_PROBES)
            run_now(args, out)


if __name__ == "__main__":
    main()
