// Where a workspace (issue 251), a skill (issue 22) or a project (issue 467) lies, as people read it.

import type {ScopeKind} from './types';

/** Where something lies, as people read it. */
export function scopeLabel(found: {scope: ScopeKind}): string {
  switch (found.scope) {
    case 'session': return 'of one session';
    case 'global': return 'global';
    case 'project': return 'of one project';
    default: return 'yours';
  }
}
