// Where something kept in a scope lies - a workspace (issue 251), a skill (issue 22), a project
// (issue 467), a team, an agent -, as people read it: whose it is where it is a user's (issue 475).

import type {ScopeKind, ScopeResponse} from './types';

/**
 * Where something of a scope of kind `kind` lies, as people read it, one of a user's scope read as
 * the reader's own: what a template names, which is made in the scope of whoever uses it.
 */
export function kindLabel(kind: ScopeKind): string {
  switch (kind) {
    case 'session': return 'of one session';
    case 'global': return 'global';
    case 'project': return 'of one project';
    default: return 'yours';
  }
}

/** Whether `found` lies in the scope of the user `userId`: theirs. */
export function isOwn(found: ScopeResponse, userId: string): boolean {
  return found.scope === 'user' && found.user_id === userId;
}

/** Where `found` lies, as the user `userId` reads it: of a user's scope, "yours" or whose it is. */
export function scopeLabel(found: ScopeResponse, userId: string): string {
  if (found.scope !== 'user' || isOwn(found, userId)) return kindLabel(found.scope);
  return found.user_name ? `of ${found.user_name}` : "another user's";
}
