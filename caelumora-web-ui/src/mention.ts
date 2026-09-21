// `@`-mentions (issue 29). Mirrors `caelumora_model::mention`: the server
// resolves mentions and stores them in `Message.mentions`; the UI only needs
// to find them in the text again to highlight the resolved ones, and to
// derive handles for completion.

import type {ActorId, ChannelMember, Message} from './types';

const HANDLE_CHAR = /[\p{L}\p{N}_-]/u;

function isHandleChar(c: string): boolean {
  return HANDLE_CHAR.test(c);
}

/** The handle a participant called `name` is mentioned by, e.g. "Dr. Who" -> "dr-who". */
export function handleOf(name: string): string {
  let handle = '';
  for (const c of name) {
    if (isHandleChar(c)) {
      handle += c.toLowerCase();
    } else if (!handle.endsWith('-')) {
      handle += '-';
    }
  }
  return handle.replace(/^-+|-+$/g, '');
}

export interface MentionSpan {
  start: number;
  end: number;
  handle: string;
}

/**
 * Finds the mentions in a piece of plain text: an `@` not preceded by a
 * handle character or a `.` (so email addresses do not count), followed by
 * handle characters, without trailing `-`. Code is excluded by the markdown
 * parser before this is called, so backticks need no handling here.
 */
export function mentionSpans(text: string): MentionSpan[] {
  const spans: MentionSpan[] = [];
  const pattern = /@([\p{L}\p{N}_-]+)/gu;
  for (const match of text.matchAll(pattern)) {
    const start = match.index;
    const prev = start > 0 ? text[start - 1] : '';
    if (prev && (isHandleChar(prev) || prev === '.')) continue;
    const raw = match[1].replace(/-+$/, '');
    if (!raw) continue;
    spans.push({start, end: start + 1 + raw.length, handle: raw.toLowerCase()});
  }
  return spans;
}

/** A stable string key for an `ActorId`, which serializes as e.g. `{"Agent": "<uuid>"}`. */
export function actorKey(id: ActorId): string {
  if (typeof id === 'string') return id;
  const [kind, value] = Object.entries(id)[0] ?? ['', ''];
  return `${kind}:${value}`;
}

/** The handles of the members a message mentions, i.e. the mentions to highlight. */
export function mentionedHandles(message: Message, members: ChannelMember[]): Set<string> {
  const keys = new Set((message.mentions ?? []).map(actorKey));
  return new Set(members.flatMap(m => (m.handle && keys.has(actorKey(m.id)) ? [m.handle] : [])));
}

/**
 * The mention being typed at the caret, if any: the partial handle after the
 * `@` and where the `@` starts.
 */
export function mentionQueryAt(text: string, caret: number): {query: string; start: number} | null {
  const match = /(^|[^\p{L}\p{N}_.-])@([\p{L}\p{N}_-]*)$/u.exec(text.slice(0, caret));
  if (!match) return null;
  return {query: match[2], start: caret - match[2].length - 1};
}
