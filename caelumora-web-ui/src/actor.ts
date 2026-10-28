// What an actor is, as an `ActorId` names it.

import type {ActorId} from './types';

/** The user `id` names, or `null` for an actor that is no user. */
export function userIdOf(id: ActorId): string | null {
  return typeof id !== 'string' && typeof id.User === 'string' ? id.User : null;
}
