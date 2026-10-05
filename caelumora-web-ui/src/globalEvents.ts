// What the session list does with an event of the global stream (issue 35).
//
// Most events name a session by id alone; the list follows them by
// removing the session, taking a new name straight from the event, or
// fetching the session again (`GET /sessions/{id}`), which answers in the
// form the snapshot uses. Events that do not change what the list shows are
// ignored.

import type {GlobalEvent} from './types';
import type {Classification} from './confidentiality';

export type SessionChange =
  | {kind: 'remove'; sessionId: string}
  | {kind: 'rename'; sessionId: string; name: string}
  | {kind: 'refresh'; sessionId: string}
  | {kind: 'ignore'};

// Events that change a session's channels or their members: the session is
// fetched again. Starting and stopping change neither, and agents and users
// themselves are not shown in the list.
const REFRESHING = new Set([
  'SessionCreated',
  'SessionRestored',
  'ChannelRestored',
  'ChannelCreated',
  'ChannelUpdated',
  'ChannelDeleted',
  'AgentChannelJoined',
  'AgentChannelLeft',
  'UserChannelJoined',
  'UserChannelLeft',
]);

/** The session an event concerns: its only value, or the first of several. */
function sessionIdOf(value: unknown): string | null {
  if (typeof value === 'string') return value;
  if (Array.isArray(value) && typeof value[0] === 'string') return value[0];
  return null;
}

/** What the session list does with `event`. */
export function sessionChange(event: GlobalEvent): SessionChange {
  const [kind, value] = Object.entries(event)[0] ?? [];
  if (!kind) return {kind: 'ignore'};
  const sessionId = sessionIdOf(value);
  if (!sessionId) return {kind: 'ignore'};

  if (kind === 'SessionDeleted') return {kind: 'remove', sessionId};
  if (kind === 'SessionUpdated') {
    const update = Array.isArray(value) ? value[1] : undefined;
    const name = update && typeof update === 'object' ? (update as {NameChanged?: unknown}).NameChanged : undefined;
    return typeof name === 'string' ? {kind: 'rename', sessionId, name} : {kind: 'refresh', sessionId};
  }
  if (REFRESHING.has(kind)) return {kind: 'refresh', sessionId};
  return {kind: 'ignore'};
}

// The global stream is opened once, by the session list; other views hear its
// events through here instead of opening a stream of their own.
type GlobalEventListener = (event: GlobalEvent) => void;
const listeners = new Set<GlobalEventListener>();

/** Calls `listener` with every event of the global stream until the returned function is called. */
export function onGlobalEvent(listener: GlobalEventListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Hands `event` of the global stream to every listener. */
export function dispatchGlobalEvent(event: GlobalEvent): void {
  listeners.forEach(listener => listener(event));
}

const MEMBERSHIP = new Set(['AgentChannelJoined', 'AgentChannelLeft', 'UserChannelJoined', 'UserChannelLeft']);

/**
 * The channel someone joined or left (issue 46), for the membership events;
 * `null` for any other event. Whoever shows the members fetches them again.
 */
export function membershipChangedChannel(event: GlobalEvent): string | null {
  const [kind, value] = Object.entries(event)[0] ?? [];
  if (!kind || !MEMBERSHIP.has(kind)) return null;
  return Array.isArray(value) && typeof value[1] === 'string' ? value[1] : null;
}

/**
 * The channel whose state of an agent changed, for `AgentChannelStateChanged`
 * (issue 65) - its notes, say; `null` for any other event. The event carries
 * no content: whoever shows the notes fetches them again.
 */
export function changedChannel(event: GlobalEvent): string | null {
  const value = event.AgentChannelStateChanged;
  return Array.isArray(value) && typeof value[1] === 'string' ? value[1] : null;
}

/**
 * The channel that was reclassified and its classification now, for
 * `ChannelUpdated` with `ClassificationChanged` (issue 133); `null` for any
 * other event. Lowering it changed its messages too: whoever shows them
 * fetches them again.
 */
export function reclassifiedChannel(event: GlobalEvent): {channelId: string; classification: Classification} | null {
  const value = event.ChannelUpdated;
  if (!Array.isArray(value) || typeof value[1] !== 'string') return null;
  const update = value[2] as {ClassificationChanged?: Classification} | undefined;
  return update?.ClassificationChanged ? {channelId: value[1], classification: update.ClassificationChanged} : null;
}

/** The message that was reclassified, for `MessageReclassified` (issue 133); `null` for any other event. */
export function reclassifiedMessage(event: GlobalEvent): {channelId: string; messageId: string; classification: Classification} | null {
  const value = event.MessageReclassified;
  if (!Array.isArray(value) || typeof value[1] !== 'string' || typeof value[2] !== 'string') return null;
  return {channelId: value[1], messageId: value[2], classification: value[3] as Classification};
}
