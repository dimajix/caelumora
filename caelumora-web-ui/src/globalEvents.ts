// What the views do with an event of the global stream (issue 35).
//
// Every event is about an entity - a session, a channel, an agent, a user, a
// profile, a workspace, a skill, an artifact, a group - and comes in one form
// (issue 323): `{"kind", "id", "within", "change"}`. `within` is the id of
// the entity it lies in, for a kind that lies in one: a channel's session
// (issue 335). What happens to a channel's members is a custom change of the
// channel.
//
// The session list follows a session's events by removing the session,
// taking a new name straight from the event, or fetching the session again
// (`GET /sessions/{id}`), which answers in the form the snapshot uses.
// Events that do not change what the list shows are ignored.

import type {ApprovalRule, EntityChange, GlobalEvent} from './types';
import type {Classification} from './confidentiality';

export type SessionChange =
  | {kind: 'remove'; sessionId: string}
  | {kind: 'rename'; sessionId: string; name: string}
  | {kind: 'refresh'; sessionId: string}
  | {kind: 'ignore'};

/** `value`, read from the global stream, as the event it is; `null` for anything that is no event. */
export function asGlobalEvent(value: unknown): GlobalEvent | null {
  if (typeof value !== 'object' || value === null) return null;
  const event = value as Partial<GlobalEvent>;
  if (typeof event.kind !== 'string' || typeof event.id !== 'string' || event.change === undefined) return null;
  return event as GlobalEvent;
}

/** What an update says, in the form of the kind's update event; `undefined` for another change. */
export function updateOf(change: EntityChange): unknown {
  return typeof change === 'object' && 'Updated' in change ? change.Updated : undefined;
}

/** What the custom change `name` says; `undefined` for another change. */
function dataOf(change: EntityChange, name: string): unknown {
  return typeof change === 'object' && 'Custom' in change && change.Custom.name === name ? change.Custom.data : undefined;
}

/** A channel that `event` is about, with the session it lies in; `null` for any other event. */
function channelEvent(event: GlobalEvent): {sessionId: string; channelId: string; change: EntityChange} | null {
  if (event.kind !== 'channel' || typeof event.within !== 'string') return null;
  return {sessionId: event.within, channelId: event.id, change: event.change};
}

/** Whether `change` of a channel is someone joining or leaving it (issue 46). */
function changesMembers(change: EntityChange): boolean {
  return dataOf(change, 'joined') !== undefined || dataOf(change, 'left') !== undefined;
}

/** What the session list does with `event`. */
export function sessionChange(event: GlobalEvent): SessionChange {
  // A change of a session's channels or their members: the session is
  // fetched again. Their start and stop, and an agent's state in them, do not
  // change what the list shows.
  const channel = channelEvent(event);
  if (channel) {
    const {sessionId, change} = channel;
    const changesChannels = typeof change === 'string' || updateOf(change) !== undefined || changesMembers(change);
    return changesChannels ? {kind: 'refresh', sessionId} : {kind: 'ignore'};
  }

  if (event.kind !== 'session') return {kind: 'ignore'};
  const {id: sessionId, change} = event;
  if (change === 'Deleted') return {kind: 'remove', sessionId};
  if (change === 'Created' || change === 'Restored') return {kind: 'refresh', sessionId};
  const update = updateOf(change);
  if (update === undefined) return {kind: 'ignore'};
  const name = update && typeof update === 'object' ? (update as {NameChanged?: unknown}).NameChanged : undefined;
  return typeof name === 'string' ? {kind: 'rename', sessionId, name} : {kind: 'refresh', sessionId};
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

/**
 * The channel someone joined or left (issue 46), for a channel's `joined` and
 * `left`; `null` for any other event. Whoever shows the members fetches them
 * again.
 */
export function membershipChangedChannel(event: GlobalEvent): string | null {
  const channel = channelEvent(event);
  return channel && changesMembers(channel.change) ? channel.channelId : null;
}

/**
 * The channel whose state of an agent changed, for a channel's
 * `agent_state_changed` (issue 65) - its notes, say; `null` for any other
 * event. The event carries no content: whoever shows the notes fetches them
 * again.
 */
export function changedChannel(event: GlobalEvent): string | null {
  return changedAgentState(event)?.channelId ?? null;
}

/**
 * Whose state changed where, for a channel's `agent_state_changed`; `null`
 * for any other event - for what follows one agent across a session (issue
 * 233).
 */
export function changedAgentState(event: GlobalEvent): {sessionId: string; channelId: string; agentId: string} | null {
  const channel = channelEvent(event);
  const agentId = channel && dataOf(channel.change, 'agent_state_changed');
  if (!channel || typeof agentId !== 'string') return null;
  return {sessionId: channel.sessionId, channelId: channel.channelId, agentId};
}

/** The channel that was changed and what its update says, for a channel's update; `null` for any other event. */
function channelUpdate(event: GlobalEvent): {channelId: string; update: object} | null {
  const channel = channelEvent(event);
  const update = channel && updateOf(channel.change);
  if (!channel || typeof update !== 'object' || update === null) return null;
  return {channelId: channel.channelId, update};
}

/**
 * The channel that was reclassified and its classification now, for a
 * channel's update with `ClassificationChanged` (issue 133); `null` for any
 * other event. Lowering it changed its messages too: whoever shows them
 * fetches them again.
 */
export function reclassifiedChannel(event: GlobalEvent): {channelId: string; classification: Classification} | null {
  const changed = channelUpdate(event);
  const classification = (changed?.update as {ClassificationChanged?: Classification} | undefined)?.ClassificationChanged;
  return changed && classification ? {channelId: changed.channelId, classification} : null;
}

/**
 * The channel whose language changed and its language now - `null` for none
 * in particular - for a channel's update with `LanguageChanged` (issue 56);
 * `null` for any other event.
 */
export function languageChangedChannel(event: GlobalEvent): {channelId: string; language: string | null} | null {
  const changed = channelUpdate(event);
  if (!changed || !('LanguageChanged' in changed.update)) return null;
  return {channelId: changed.channelId, language: (changed.update as {LanguageChanged: string | null}).LanguageChanged ?? null};
}


/**
 * The channel whose description changed and what it says now, for a
 * channel's update with `DescriptionChanged` (issue 245); `null` for any
 * other event.
 */
export function descriptionChangedChannel(event: GlobalEvent): {channelId: string; summary?: string; briefing?: string} | null {
  const changed = channelUpdate(event);
  if (!changed || !('DescriptionChanged' in changed.update)) return null;
  const {summary, briefing} = (changed.update as {DescriptionChanged: {summary: string | null; briefing: string | null}}).DescriptionChanged;
  return {channelId: changed.channelId, summary: summary ?? undefined, briefing: briefing ?? undefined};
}

/**
 * The channel whose approval rules changed and its rules now, for a
 * channel's update with `ApprovalRulesChanged` (issue 13); `null` for any
 * other event.
 */
export function approvalRulesChangedChannel(event: GlobalEvent): {channelId: string; rules: ApprovalRule[]} | null {
  const changed = channelUpdate(event);
  if (!changed || !('ApprovalRulesChanged' in changed.update)) return null;
  return {channelId: changed.channelId, rules: (changed.update as {ApprovalRulesChanged: ApprovalRule[]}).ApprovalRulesChanged ?? []};
}

/**
 * The rules of the logged-in user now, for an update of a `user_profile`
 * (issue 13), which comes as the profile now stands and reaches only the user
 * whose profile it is; `null` for any other event.
 */
export function profileApprovalRules(event: GlobalEvent): ApprovalRule[] | null {
  if (event.kind !== 'user_profile') return null;
  const profile = updateOf(event.change);
  if (typeof profile !== 'object' || profile === null) return null;
  return (profile as {approval_rules?: ApprovalRule[]}).approval_rules ?? [];
}
