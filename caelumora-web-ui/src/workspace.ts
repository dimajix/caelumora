// How workspaces are shown (issue 253): labels of what the server says.

import type {WorkspaceResponse} from './types';

/** Where a workspace lies, as people read it. */
export function workspaceScopeLabel(workspace: WorkspaceResponse): string {
  switch (workspace.scope) {
    case 'session': return 'of one session';
    case 'global': return 'global';
    default: return 'yours';
  }
}
