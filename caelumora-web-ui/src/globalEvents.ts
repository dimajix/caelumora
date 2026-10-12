// What the views do with an event of the global stream (issue 35).
//
// An event about an entity named by its own id - a session, an agent, a
// user, a profile, a workspace, a skill, an artifact, a group - comes in one
// form (issue 323): `{"Entity": {"kind", "id", "change"}}`. The events of a
// channel and of its members name the session too, and keep a variant each.
//
// The session list follows a session's events by removing the session,
// taking a new name straight from the event, or fetching the session again
// (`GET /sessions/{id}`), which answers in the form the snapshot uses.
// Events that do not change what the list shows are ignored.

import type {ApprovalRule, GlobalEvent} from './types';
import type {Classification} from './confidentiality';

export type SessionChange =
  | {kind: 'remove'; sessionId: string}
  | {kind: 'rename'; sessionId: string; name: string}
  | {kind: 'refresh'; sessionId: string}
  | {kind: 'ignore'};

/**
 * What happened to an entity: one of the four of its life, the same for
 * every kind, or something particular to its kind under a name of its own -
 * a session `started`, an agent's `state_changed`.
 */
export type EntityChange =
  | 'Created'
  | 'Deleted'
  | 'Restored'
  | {Updated: unknown}
  | {Custom: {name: string; data: unknown}};

/** An event about an entity, named by its kind and its own id. */
export interface EntityEvent {
  kind: string;
  id: string;
  change: EntityChange;
}

/** The entity `event` is about and what happened to it; `null` for an event of a channel or its members. */
export function entityEvent(event: GlobalEvent): EntityEvent | null {
  const entity = event.Entity as Partial<EntityEvent> | undefined;
  if (!entity || typeof entity.kind !== 'string' || typeof entity.id !== 'string' || entity.change === undefined) return null;
  return entity as EntityEvent;
}

/** What an update says, in the form of the kind's update event; `undefined` for another change. */
export function updateOf(change: EntityChange): unknown {
  return typeof change === 'object' && 'Updated' in change ? change.Updated : undefined;
}

// Events that change a session's channels or their members: the session is
// fetched again. Agents and users themselves are not shown in the list.
const REFRESHING = new Set([
  'ChannelRestored',
  'ChannelCreated',
  'ChannelUpdated',
  'ChannelDeleted',
  'AgentChannelJoined',
  'AgentChannelLeft',
  'UserChannelJoined',
  'UserChannelLeft',
]);

/** The session an event of a channel concerns: the first of its values. */
function sessionIdOf(value: unknown): string | null {
  if (typeof value === 'string') return value;
  if (Array.isArray(value) && typeof value[0] === 'string') return value[0];
  return null;
}

/** What the session list does with `event`. */
export function sessionChange(event: GlobalEvent): SessionChange {
  const entity = entityEvent(event);
  if (entity) {
    if (entity.kind !== 'session') return {kind: 'ignore'};
    const {id: sessionId, change} = entity;
    if (change === 'Deleted') return {kind: 'remove', sessionId};
    if (change === 'Created' || change === 'Restored') return {kind: 'refresh', sessionId};
    const update = updateOf(change);
    if (update === undefined) return {kind: 'ignore'};
    const name = update && typeof update === 'object' ? (update as {NameChanged?: unknown}).NameChanged : undefined;
    return typeof name === 'string' ? {kind: 'rename', sessionId, name} : {kind: 'refresh', sessionId};
  }

  const [kind, value] = Object.entries(event)[0] ?? [];
  if (!kind) return {kind: 'ignore'};
  const sessionId = sessionIdOf(value);
  if (!sessionId) return {kind: 'ignore'};
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
  return changedAgentState(event)?.channelId ?? null;
}

/**
 * Whose state changed where, for `AgentChannelStateChanged`; `null` for any
 * other event - for what follows one agent across a session (issue 233).
 */
export function changedAgentState(event: GlobalEvent): {sessionId: string; channelId: string; agentId: string} | null {
  const value = event.AgentChannelStateChanged;
  if (!Array.isArray(value) || !value.every(id => typeof id === 'string')) return null;
  const [sessionId, channelId, agentId] = value as string[];
  return {sessionId, channelId, agentId};
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

/**
 * The channel whose language changed and its language now - `null` for none
 * in particular - for `ChannelUpdated` with `LanguageChanged` (issue 56);
 * `null` for any other event.
 */
export function languageChangedChannel(event: GlobalEvent): {channelId: string; language: string | null} | null {
  const value = event.ChannelUpdated;
  if (!Array.isArray(value) || typeof value[1] !== 'string') return null;
  const update = value[2];
  if (typeof update !== 'object' || update === null || !('LanguageChanged' in update)) return null;
  return {channelId: value[1], language: (update as {LanguageChanged: string | null}).LanguageChanged ?? null};
}


/**
 * The channel whose description changed and what it says now, for
 * `ChannelUpdated` with `DescriptionChanged` (issue 245); `null` for any
 * other event.
 */
export function descriptionChangedChannel(event: GlobalEvent): {channelId: string; summary?: string; briefing?: string} | null {
  const value = event.ChannelUpdated;
  if (!Array.isArray(value) || typeof value[1] !== 'string') return null;
  const update = value[2];
  if (typeof update !== 'object' || update === null || !('DescriptionChanged' in update)) return null;
  const {summary, briefing} = (update as {DescriptionChanged: {summary: string | null; briefing: string | null}}).DescriptionChanged;
  return {channelId: value[1], summary: summary ?? undefined, briefing: briefing ?? undefined};
}

/**
 * The channel whose approval rules changed and its rules now, for
 * `ChannelUpdated` with `ApprovalRulesChanged` (issue 13); `null` for any
 * other event.
 */
export function approvalRulesChangedChannel(event: GlobalEvent): {channelId: string; rules: ApprovalRule[]} | null {
  const value = event.ChannelUpdated;
  if (!Array.isArray(value) || typeof value[1] !== 'string') return null;
  const update = value[2];
  if (typeof update !== 'object' || update === null || !('ApprovalRulesChanged' in update)) return null;
  return {channelId: value[1], rules: (update as {ApprovalRulesChanged: ApprovalRule[]}).ApprovalRulesChanged ?? []};
}

/**
 * The rules of the logged-in user now, for an update of a `user_profile`
 * (issue 13), which comes as the profile now stands and reaches only the user
 * whose profile it is; `null` for any other event.
 */
export function profileApprovalRules(event: GlobalEvent): ApprovalRule[] | null {
  const entity = entityEvent(event);
  if (entity?.kind !== 'user_profile') return null;
  const profile = updateOf(entity.change);
  if (typeof profile !== 'object' || profile === null) return null;
  return (profile as {approval_rules?: ApprovalRule[]}).approval_rules ?? [];
}
