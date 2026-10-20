#!/usr/bin/env python3
"""A channel of a Caelumora JSON store, exported for experiments with its turns.

Reads what the `JsonFileStore` keeps of one channel - its history, and for
each agent its transcript and what its model was given (`<agent>.given/`:
prompt texts, tool sets, frames, issue 426) - and writes it into one file,
without what an experiment does not need (security contexts, durations,
classifications of messages). From such a file it rebuilds, for every turn of
an agent, the request its model was sent for the turn's first call, as the
`ContextResolver` (`caelumora-model/src/context.rs`) and the Ollama client
(`caelumora-ollama/src/client.rs`) build it. That request is what an
experiment varies - a prompt part replaced, one added - and sends again.

Every turn is told with why the agent was asked (the requests to respond the
history records before it, and whether the message that asked mentioned the
agent) and what it did (what it said, which tools it called, whether it
stayed silent).

Usage:
    python3 channel_export.py export --store ../../local --channel a7b27210-... -o team-session.json
    python3 channel_export.py show team-session.json            # the channel as it was said
    python3 channel_export.py turns team-session.json           # every turn, why asked, what done
    python3 channel_export.py request team-session.json Coder 3 # the request of a turn, as JSON
    python3 channel_export.py verify team-session.json          # rebuilt requests against Ollama

`verify` sends each rebuilt request with one token to generate and compares
how many tokens Ollama counts in it with how many the turn's first call was
measured with (`prompt_tokens`): the same number, the same request.
"""

import argparse
import datetime as dt
import json
import pathlib
import re
import sys
import unicodedata
import urllib.request
import zoneinfo

# The proxy of the machine refuses localhost: no proxy for Ollama.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
OLLAMA = "http://127.0.0.1:11434"


# --- Reading the store -------------------------------------------------------

def actor_key(actor):
    """`{"Agent": id}` / `{"User": id}` / `"System"` as one string."""
    if isinstance(actor, dict):
        (kind, id_), = actor.items()
        return f"{kind.lower()}:{id_}"
    return str(actor).lower()


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def history_items(directory):
    """The items of a channel's history, in order: `items-<n>.json` hold `[seq, item]` pairs."""
    items = []
    for path in directory.glob("items-*.json"):
        if path.stem != "items-head":
            items += read_json(path)
    return [item for _, item in sorted(items, key=lambda pair: pair[0])]


def active_path(directory):
    """The active path of a chain kept in `chain-<n>.json`, from its head back to its root."""
    nodes = {}
    for path in directory.glob("chain-*.json"):
        for node in read_json(path):
            nodes[node["digest"]] = node
    head = read_json(directory / "head.json").get("head")
    path = []
    while head:
        node = nodes[head]
        path.append(node)
        head = node["parent"]
    return path[::-1]


def simple_message(message):
    """A message as the export keeps it: what the model is given of it, and what tells about it."""
    kept = {
        "id": message["id"],
        "at": message["timestamp"],
        "sender": actor_key(message["sender_id"]),
        "sender_name": message.get("sender_name", ""),
        "role": message["role"],
    }
    for key in ("content", "reasoning", "turn_id", "prompt_tokens", "revoked"):
        if message.get(key) is not None:
            kept[key] = message[key]
    if message.get("internal"):
        kept["internal"] = True
    if message.get("mentions"):
        kept["mentions"] = [actor_key(m) for m in message["mentions"]]
    if message.get("tool_calls"):
        kept["tool_calls"] = [{"id": c.get("id"), "name": c["function_name"], "arguments": c["arguments"]}
                              for c in message["tool_calls"]]
    if message.get("tool_results"):
        # What the model is given of a result (`ToolCallResult::for_the_model`):
        # all of it but the answers and the approval.
        kept["tool_results"] = [{k: v for k, v in r.items() if k not in ("answers", "approval")}
                                for r in message["tool_results"]]
    return kept


def simple_entry(node):
    """A transcript entry as the export keeps it: its digest, its kind and what it holds."""
    (kind, body), = node["entry"].items() if isinstance(node["entry"], dict) else ((node["entry"], {}),)
    entry = {"digest": node["digest"], "kind": kind}
    if kind == "message":
        entry["message"] = simple_message(body)
    elif kind == "given":
        entry["message"] = simple_message(body["message"])
    elif kind == "modified":
        entry["message"] = simple_message(body["message"])
    elif kind in ("revoked", "reclassified"):
        entry["of"] = body["message"]
        entry["at"] = body.get("at")
    elif kind in ("participant_joined", "participant_left"):
        entry.update(participant=actor_key(body["participant"]), name=body["name"],
                     by=actor_key(body["by"]), by_name=body["by_name"], at=body["at"])
    elif kind in ("turn_finished", "turn_given_up"):
        entry.update(turn_id=body["turn_id"], at=body["at"])
        if kind == "turn_given_up":
            entry["reason"] = next(iter(body["reason"])) if isinstance(body["reason"], dict) else body["reason"]
    elif kind == "compacted":
        entry["at"] = body.get("at")
    return entry


def simple_history_item(item):
    """A history item as the export keeps it."""
    (kind, body), = item["entry"].items() if isinstance(item["entry"], dict) else ((item["entry"], {}),)
    kept = {"kind": kind, "at": item["timestamp"], "sender": actor_key(item["sender"]),
            "event_id": item["context"]["event_id"], "agent_hops": item["context"].get("agent_hops", 0)}
    recipients = item.get("recipients") or {}
    if "named" in recipients:
        kept["to"] = [actor_key(a) for a in recipients["named"]]
    if kind == "said":
        message = body["message"]
        kept["message"] = simple_message(message)
        kept["context"] = {"transcript": message.get("context_transcript"), "frame": message.get("context_frame")}
    elif kind == "response_requested":
        kept["by"] = actor_key(body["by"])
    elif kind in ("participant_joined", "participant_left"):
        kept.update(participant=actor_key(body["participant"]), name=body["name"])
    elif kind == "turn_finished":
        kept.update(turn_id=body["turn_id"], actor=actor_key(body["actor"]))
    elif kind == "chain_cut_off":
        kept.update(message=body["message"], not_asked=[actor_key(a) for a in body["not_asked"]])
    elif kind == "plan_step_started":
        kept.update(actor=actor_key(body["actor"]), step=body["step_index"], description=body["description"])
    return kept


def export(store, channel_id):
    state = store / "state_store" / "runtime"
    entities = store / "entity_store" / "entities"
    channel = read_json(entities / "channel" / f"{channel_id}.json")
    session = read_json(entities / "session" / f"{channel['session_id']}.json")

    history = [simple_history_item(item)
               for item in history_items(state / "channel_state" / f"{channel_id}.history")]

    # Names as the history tells them where someone joined, else from the entities.
    names = {h["participant"]: h["name"] for h in history if h["kind"] == "participant_joined"}
    participants = {}
    for actor in channel["participants"]:
        key = actor_key(actor)
        (kind, id_), = actor.items()
        if key not in names:
            entity = read_json(entities / kind.lower() / f"{id_}.json")
            names[key] = entity.get("name") or entity.get("username")
        participants[key] = {"name": names[key], "kind": kind.lower()}

    agents = {}
    for key, participant in participants.items():
        if participant["kind"] != "agent":
            continue
        id_ = key.split(":", 1)[1]
        base = state / "agent_channel_state" / channel_id
        given = base / f"{id_}.given"
        prompts, tools, frames = {}, {}, {}
        for path in given.glob("*.json"):
            kind, digest = path.stem.split("-", 1)
            {"prompt": prompts, "tools": tools, "frame": frames}[kind][digest] = read_json(path)
        agents[participant["name"]] = {
            "id": key,
            "transcript": [simple_entry(node) for node in active_path(base / f"{id_}.transcript")],
            "prompts": prompts,
            "tools": tools,
            "frames": frames,
        }

    data = {
        "source": {"session": session["name"], "session_id": session["id"], "channel": channel["name"],
                   "channel_id": channel_id, "exported_at": dt.datetime.now(dt.timezone.utc).isoformat()},
        "briefing": session.get("briefing"),
        "components": channel.get("components", []),
        "participants": participants,
        "history": history,
        "agents": agents,
    }
    for name, agent in agents.items():
        agent["turns"] = find_turns(data, name)
    return data


# --- Turns ---------------------------------------------------------------------

def parse_time(text):
    """A time as the store writes it - in UTC, with up to nine digits of a second."""
    text = re.sub(r"(\.\d{6})\d*", r"\1", text).replace("Z", "+00:00")
    return dt.datetime.fromisoformat(text)


def find_turns(data, name):
    """The turns of agent `name`: where each began in its transcript, the frame of its first
    call, why it was asked and what it did."""
    agent = data["agents"][name]
    me = agent["id"]
    transcript = agent["transcript"]
    position = {entry["digest"]: at for at, entry in enumerate(transcript)}
    said_contexts = {}
    for h in data["history"]:
        if h["kind"] == "said" and h["sender"] == me and h["context"]["transcript"]:
            said_contexts.setdefault(h["message"].get("turn_id"), []).append(h["context"])
    requests = [h for h in data["history"] if h["kind"] == "response_requested" and me in h.get("to", [])]
    # Each request with the messages that asked: a person's message asks as it is said, an
    # agent's turn as it ends - with what all its messages mention.
    said = [h for h in data["history"] if h["kind"] == "said"]
    by_event = {h["event_id"]: [h["message"]] for h in said}
    for h in data["history"]:
        if h["kind"] == "turn_finished":
            by_event[h["event_id"]] = [m["message"] for m in said if m["message"].get("turn_id") == h["turn_id"]]
    heard = {}
    for at, e in enumerate(transcript):
        if e.get("message"):
            heard.setdefault(e["message"]["id"], at)  # first heard: a replay hears it again (issue 440)

    turns, starts = [], []
    for at, entry in enumerate(transcript):
        if entry["kind"] not in ("turn_finished", "turn_given_up"):
            continue
        turn_id = entry["turn_id"]
        own = [i for i, e in enumerate(transcript[:at]) if e.get("message", {}).get("turn_id") == turn_id]
        if not own:
            continue
        start = own[0]  # the transcript before the turn's first message is what its first call saw
        before = transcript[start - 1]["digest"] if start else None
        frame, exact = first_frame(agent, before, position, said_contexts.get(turn_id, []))
        turns.append({
            "turn_id": turn_id,
            "start": start,
            "transcript": before,
            "frame": frame,
            "frame_exact": exact,
            "asked": [],
            "did": what_it_did([transcript[i]["message"] for i in own]),
            "prompt_tokens": transcript[start]["message"].get("prompt_tokens"),
        })
        starts.append((start, parse_time(transcript[start]["message"]["at"])))

    # A request belongs to the first turn that began after the agent heard what asked - or, if
    # nothing it heard asked (a nudge), after the request was made.
    for request in requests:
        messages = by_event.get(request["event_id"], [])
        places = [heard[m["id"]] for m in messages if m["id"] in heard]
        if places:
            index = next((i for i, (start, _) in enumerate(starts) if start > max(places)), None)
        else:
            made = parse_time(request["at"])
            index = next((i for i, (_, began) in enumerate(starts) if began > made), None)
        if index is not None:
            turns[index]["asked"].append(describe_request(request, messages, me, data))
    return turns


def first_frame(agent, before, position, contexts):
    """The frame the turn's first call was given: as the history names it with a message the call
    produced, else the frame whose parts all stand before the turn and the last of them nearest to
    it - the part a turn places as it begins (the current time) stands after the transcript's end."""
    for context in contexts:
        if context["transcript"] == before:
            return context["frame"], True
    end = position.get(before, -1)
    best, best_at = None, None
    for digest, frame in agent["frames"].items():
        places = [position.get(p["after"], None) if p["after"] else -1 for p in frame["prompts"]]
        if any(p is None or p > end for p in places):
            continue
        last = max(places, default=-1)
        rank = (last, -len(frame.get("knowledge", [])), -len(frame["prompts"]))
        if best_at is None or rank > best_at:
            best, best_at = digest, rank
    return best, best_at is not None and best_at[0] == end


def describe_request(request, messages, me, data):
    """Why a request to respond was made: what asked mentioned the agent, `@all`, or neither - it
    was chosen; nothing said asked: it was nudged."""
    by = data["participants"].get(request["by"], {}).get("name", request["by"])
    described = {"at": request["at"], "by": by, "messages": [m["id"] for m in messages]}
    if not messages:
        described["reason"] = "nudged"
    elif any(me in m.get("mentions", []) for m in messages):
        described["reason"] = "mentioned"
    elif any("@all" in (m.get("content") or "") for m in messages):
        described["reason"] = "mentioned_all"
    else:
        described["reason"] = "chosen"
    return described


def what_it_did(messages):
    """What a turn did: what it said, which tools it called, and how it ended."""
    said = [m["content"] for m in messages if m.get("content") and m["role"] != "Tool"]
    calls = [c["name"] for m in messages for c in m.get("tool_calls", [])]
    ending = next((c for c in reversed(calls) if c in ("silence", "stop", "bye")), None)
    return {"said": said, "tools": calls, "ended": ending or ("spoke" if said else "nothing")}


# --- The model's request ---------------------------------------------------------

def handle_of(name):
    """`handle_of` of `caelumora-model/src/mention.rs`, `any_ascii` approximated by stripping accents."""
    ascii_ = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    handle = ""
    for c in ascii_:
        if c.isalnum() or c in "_-":
            handle += c.lower()
        elif not handle.endswith("-"):
            handle += "-"
    return handle.strip("-")


def sender_of(name, style):
    if style == "name":
        return name
    handle = handle_of(name)
    return f"@{handle}" if handle else name


def framed(message, me, profile, zone):
    """`ModelProfile::frame`: a message of another participant marked with its sender and time."""
    by_another = message["sender"] != me and message["sender"].split(":")[0] in ("user", "agent") \
        and message["role"] == "User"
    content = message.get("content")
    if profile.get("sender_format", "none") == "none" or not by_another or content is None:
        return content
    sender = sender_of(message["sender_name"], profile.get("sender_style", "handle"))
    at = None
    if profile.get("message_times"):
        at = parse_time(message["at"]).astimezone(zone).strftime("%a %-d %b %Y, %H:%M")
    if profile["sender_format"] == "prefix":
        return f"{sender} ({at}): {content}" if at else f"{sender}: {content}"
    sender = sender.replace("&", "&amp;").replace('"', "&quot;")
    content = content.replace("</message", "&lt;/message").replace("<message", "&lt;message")
    return f'<message from="{sender}"{f" at=\"{at}\"" if at else ""}>{content}</message>'


def ollama_messages(message, me, profile, zone, in_turn):
    """`OllamaChatMessage::from` of a message as `ModelProfile::given` gives it."""
    keeps_reasoning = message["sender"] == me and {
        "never": False, "current_turn": in_turn, "always": True}[profile.get("reasoning_history", "current_turn")]
    content = framed(message, me, profile, zone)
    reasoning = message.get("reasoning") if keeps_reasoning else None
    out = []
    if message.get("tool_calls") or content is not None or reasoning is not None:
        m = {"role": message["role"].lower(), "content": content or ""}
        if reasoning is not None:
            m["thinking"] = reasoning
        if message.get("tool_calls"):
            m["tool_calls"] = [{"id": c["id"], "function": {"index": i, "name": c["name"], "arguments": c["arguments"]}}
                               for i, c in enumerate(message["tool_calls"])]
        out.append(m)
    for r in message.get("tool_results", []):
        out.append({"role": "tool", "content": json.dumps(r, ensure_ascii=False, separators=(",", ":")),
                    "tool_name": r["tool_name"], "tool_call_id": r.get("id")})
    return out


def notice(entry, me, style):
    """`participant_notice`: who joined or left, as the channel tells the model."""
    def named(actor, name, you):
        return you if actor == me else sender_of(name, style)
    was = "were" if entry["participant"] == me else "was"
    verb = "joined" if entry["kind"] == "participant_joined" else "left"
    if entry["by"] == entry["participant"]:
        text = f"{named(entry['participant'], entry['name'], 'You')} {verb} the channel."
    elif entry["by"] == "system":
        done = "added to" if verb == "joined" else "removed from"
        text = f"{named(entry['participant'], entry['name'], 'You')} {was} {done} the channel."
    else:
        done = "added" if verb == "joined" else "removed"
        to = "to" if verb == "joined" else "from"
        text = f"{named(entry['by'], entry['by_name'], 'You')} {done} {named(entry['participant'], entry['name'], 'you')} {to} the channel."
    return {"role": "system", "content": f"Channel notice: {text}"}


GIVEN_UP = {
    "Failed": "it failed", "Lowered": "the channel's classification was lowered", "Left": "you left the channel",
    "Retried": "your answer was asked for again",
    "CallLimit": "it asked the model as often as one answer, or one step of a plan, may",
}


def ollama_parameters(parameters):
    """`OllamaFunctionParameters::from`: only what the client sends of a tool's parameters."""
    kind = parameters["type"]
    out = {"type": kind}
    if kind == "object":
        out["properties"] = {k: ollama_parameters(v) for k, v in parameters.get("properties", {}).items()}
        if parameters.get("required"):
            out["required"] = parameters["required"]
    elif kind == "array":
        out["items"] = ollama_parameters(parameters["items"])
    if kind != "object" and parameters.get("description") is not None:
        out["description"] = parameters["description"]
    return out


def ollama_tools(tool_sets):
    return [{"type": "function", "function": {"name": t["name"], "description": t["description"],
                                              "parameters": ollama_parameters(t["parameters"])}}
            for tool_set in tool_sets for t in tool_set["tools"]]


def entry_key(entry):
    """What tells an entry heard again from the first time it was heard: its message's id, or who
    joined or left when."""
    if entry.get("message"):
        return entry["message"]["id"]
    if entry["kind"] in ("participant_joined", "participant_left"):
        return (entry["kind"], entry["participant"], entry["at"])
    return None


def turn_context(data, name, turn, heard_once=False):
    """What the first call of `turn` of agent `name` was given: its prompt parts with the place
    each stood in (`None`: before everything, else after the transcript entry of that index), the
    transcript before the turn, the tool sets and the model's profile.

    `heard_once` leaves out what the agent heard again - the history replayed after a restart,
    before issue 440 kept where an agent stood in it - and moves a part that stood after such an
    entry behind the last entry kept before it."""
    agent = data["agents"][name]
    frame = agent["frames"][turn["frame"]]
    transcript = agent["transcript"][:turn["start"]]
    kept, seen, place = [], set(), {}
    for at, entry in enumerate(transcript):
        key = entry_key(entry)
        if heard_once and key is not None and key in seen:
            place[entry["digest"]] = len(kept) - 1 if kept else None
            continue
        seen.add(key)
        place[entry["digest"]] = len(kept)
        kept.append(entry)
    parts = [{"role": p["role"], "text": agent["prompts"][p["text"]],
              "after": place[p["after"]] if p["after"] else None} for p in frame["prompts"]]
    tool_sets = agent["tools"][frame["tools"]] if frame.get("tools") else []
    return {"parts": parts, "transcript": kept, "tool_sets": tool_sets,
            "profile": frame["profile"], "time_zone": frame.get("time_zone", "UTC"), "me": agent["id"]}


def model_request(context, model, think=None, num_ctx=65536, turn_id=None):
    """The body of `/api/chat` for `context` (`turn_context`, maybe changed by an experiment), as
    `ContextResolver::resolve` and the Ollama client build it."""
    profile, me = context["profile"], context["me"]
    zone = zoneinfo.ZoneInfo(context["time_zone"])
    style = profile.get("sender_style", "handle")
    changes = {}
    for entry in context["transcript"]:
        if entry["kind"] in ("modified", "revoked"):
            changes.setdefault(entry.get("of") or entry["message"]["id"], []).append(entry)

    def parts_after(after):
        return [{"role": p["role"].lower(), "content": p["text"]} for p in context["parts"] if p["after"] == after]

    messages = parts_after(None)
    for at, entry in enumerate(context["transcript"]):
        message = entry.get("message") if entry["kind"] in ("message", "given") else None
        if message is not None:
            for change in changes.get(message["id"], []):
                message = None if change["kind"] == "revoked" else {**message, **change["message"]}
                if message is None:
                    break
        if message is not None:
            messages += ollama_messages(message, me, profile, zone, message.get("turn_id") == turn_id)
        if entry["kind"] == "turn_given_up":
            messages.append({"role": "system", "content":
                             f"Your previous turn was given up before it was complete, because "
                             f"{GIVEN_UP.get(entry['reason'], 'it failed')}. What its tool calls did stays done: "
                             f"do not repeat them unless that is what is wanted."})
        if entry["kind"] in ("participant_joined", "participant_left"):
            messages.append(notice(entry, me, style))
        messages += parts_after(at)

    # `ModelProfile::place_system_messages`: only the leading system messages stay ones.
    if profile.get("system_messages", "only_at_start") == "only_at_start":
        leading = 0
        while leading < len(messages) and messages[leading]["role"] == "system":
            leading += 1
        for m in messages[leading:]:
            if m["role"] == "system":
                m["role"] = "user"

    body = {"model": model, "messages": messages, "stream": False, "options": {"num_ctx": num_ctx}}
    tools = ollama_tools(context["tool_sets"])
    if tools:
        body["tools"] = tools
    if think is not None:
        body["think"] = think
    return body


def chat(body, timeout=900):
    request = urllib.request.Request(f"{OLLAMA}/api/chat", json.dumps(body).encode(), {"Content-Type": "application/json"})
    with OPENER.open(request, timeout=timeout) as response:
        return json.load(response)


# --- Commands ----------------------------------------------------------------------

def load(path):
    return read_json(path)


def short(text, width=100):
    text = " ".join((text or "").split())
    return text if len(text) <= width else text[:width - 1] + "…"


def cmd_export(args):
    data = export(pathlib.Path(args.store), args.channel)
    with open(args.output, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=1)
    turns = sum(len(a["turns"]) for a in data["agents"].values())
    print(f"{args.output}: {len(data['history'])} history items, {len(data['agents'])} agents, {turns} turns")


def cmd_show(args):
    data = load(args.file)
    names = {k: v["name"] for k, v in data["participants"].items()}
    for h in data["history"]:
        if h["kind"] == "said":
            m = h["message"]
            print(f"[{m['at'][:19]}] {m['sender_name']}: {m.get('content', '')}\n")
        elif h["kind"] == "response_requested" and args.requests:
            print(f"    (asked: {', '.join(names.get(a, a) for a in h['to'])}, by {names.get(h['by'], h['by'])})")
        elif h["kind"] == "chain_cut_off" and args.requests:
            print(f"    (chain cut off, not asked: {', '.join(names.get(a, a) for a in h['not_asked'])})")


def cmd_turns(args):
    data = load(args.file)
    for name, agent in data["agents"].items():
        if args.agent and name != args.agent:
            continue
        for index, turn in enumerate(agent["turns"]):
            asked = ", ".join(f"{a['reason']} by {a['by']}" for a in turn["asked"]) or "-"
            said = short(" | ".join(turn["did"]["said"]), 80)
            flag = "" if turn["frame_exact"] else " (frame guessed)"
            print(f"{name:>9} {index:>3}  asked: {asked:<40} ended: {turn['did']['ended']:<8} {said}{flag}")


def cmd_request(args):
    data = load(args.file)
    turn = data["agents"][args.agent]["turns"][args.turn]
    body = model_request(turn_context(data, args.agent, turn, args.heard_once), args.model, turn_id=turn["turn_id"])
    json.dump(body, sys.stdout, ensure_ascii=False, indent=1)
    print()


def cmd_verify(args):
    data = load(args.file)
    same = checked = 0
    for name, agent in data["agents"].items():
        for index, turn in enumerate(agent["turns"]):
            if not turn["frame"] or turn["prompt_tokens"] is None:
                continue
            if args.limit and checked >= args.limit:
                break
            body = model_request(turn_context(data, name, turn), args.model, turn_id=turn["turn_id"])
            body["options"]["num_predict"] = 1
            counted = chat(body).get("prompt_eval_count")
            checked += 1
            same += counted == turn["prompt_tokens"]
            difference = "same" if counted == turn["prompt_tokens"] else f"{counted - turn['prompt_tokens']:+}"
            print(f"{name:>9} {index:>3}  recorded {turn['prompt_tokens']:>6}  rebuilt {counted:>6}  {difference}")
    print(f"{same} of {checked} the same")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("export", help="export a channel of a JSON store")
    p.add_argument("--store", required=True, help="the directory holding state_store/ and entity_store/")
    p.add_argument("--channel", required=True, help="the channel's id")
    p.add_argument("-o", "--output", required=True)
    p.set_defaults(run=cmd_export)
    p = commands.add_parser("show", help="the channel as it was said")
    p.add_argument("file")
    p.add_argument("--requests", action="store_true", help="also who was asked to respond")
    p.set_defaults(run=cmd_show)
    p = commands.add_parser("turns", help="every turn: why asked, what done")
    p.add_argument("file")
    p.add_argument("--agent")
    p.set_defaults(run=cmd_turns)
    p = commands.add_parser("request", help="the request of a turn's first call")
    p.add_argument("file")
    p.add_argument("agent")
    p.add_argument("turn", type=int)
    p.add_argument("--model", default="gemma4:26b")
    p.add_argument("--heard-once", action="store_true", help="without what the agent heard again (issue 440)")
    p.set_defaults(run=cmd_request)
    p = commands.add_parser("verify", help="count the rebuilt requests' tokens with Ollama")
    p.add_argument("file")
    p.add_argument("--model", default="gemma4:26b")
    p.add_argument("--limit", type=int, default=0)
    p.set_defaults(run=cmd_verify)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
