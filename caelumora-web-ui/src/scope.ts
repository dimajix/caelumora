// Where a workspace (issue 251) or a skill (issue 22) lies, as people read it.

import type {ScopeKind} from './types';

/** Where something lies, as people read it. */
export function scopeLabel(found: {scope: ScopeKind}): string {
  switch (found.scope) {
    case 'session': return 'of one session';
    case 'global': return 'global';
    default: return 'yours';
  }
}
