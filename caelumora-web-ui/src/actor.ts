// What an actor is, as an `ActorId` names it: the one place that knows the
// form serde gives an `ActorId` (issue 474), to read one and to make one.

import type {ActorId} from './types';

/** The actor of the user `userId`. */
export function userActor(userId: string): ActorId {
  return {User: userId};
}

/** The actor of the agent `agentId`. */
export function agentActor(agentId: string): ActorId {
  return {Agent: agentId};
}

/** The user `id` names, or `null` for an actor that is no user. */
export function userIdOf(id: ActorId): string | null {
  return typeof id !== 'string' && 'User' in id ? id.User : null;
}

/** The agent `id` names, or `null` for an actor that is no agent. */
export function agentIdOf(id: ActorId): string | null {
  return typeof id !== 'string' && 'Agent' in id ? id.Agent : null;
}

/** Whether `id` is an approver. */
export function isApprover(id: ActorId): boolean {
  return typeof id !== 'string' && 'Approver' in id;
}

/** Whether `id` is the runtime itself, acting on no one's behalf. */
export function isSystem(id: ActorId): boolean {
  return id === 'System';
}

/**
 * A stable string key for an `ActorId`, e.g. `Agent:<uuid>`, for keys that are kept; two actors
 * are compared with `sameActor`.
 */
export function actorKey(id: ActorId): string {
  if (typeof id === 'string') return id;
  const [kind, value] = Object.entries(id)[0] ?? ['', ''];
  return `${kind}:${value}`;
}

/** Whether `a` and `b` name the same actor. */
export function sameActor(a: ActorId, b: ActorId): boolean {
  return actorKey(a) === actorKey(b);
}

/** The part of an `ActorId` the REST API addresses an actor by. */
export function actorUuid(id: ActorId | undefined): string | null {
  if (!id || typeof id === 'string') return null;
  return Object.values(id)[0] ?? null;
}
