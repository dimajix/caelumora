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

/** The name every approver goes by (`ActorId::APPROVER_NAME`). */
export const APPROVER_NAME = 'Approver';

/** Whether `id` is an approver. */
export function isApprover(id: ActorId): boolean {
  return typeof id !== 'string' && 'Approver' in id;
}

/** Whether `id` is the kernel itself, acting on no one's behalf. */
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

/**
 * How a path of the REST API names a user or an agent (`ActorPath`, issue 590): its `actorKey`,
 * `User:<uuid>` or `Agent:<uuid>`; `null` for any other actor, which no path names.
 */
export function actorPath(id: ActorId | undefined): string | null {
  return id && (userIdOf(id) !== null || agentIdOf(id) !== null) ? actorKey(id) : null;
}
