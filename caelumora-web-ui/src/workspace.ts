// How workspaces are shown (issue 253): labels of what the server says.

import type {StorageResponse, WorkspaceScope} from './types';

/** Where a workspace lies, as people read it. */
export function workspaceScopeLabel(workspace: {scope: WorkspaceScope}): string {
  switch (workspace.scope) {
    case 'session': return 'of one session';
    case 'global': return 'global';
    default: return 'yours';
  }
}

/** A storage as people choose it: its name and the kind of workspace it makes (issue 266). */
export function storageLabel(storage: StorageResponse): string {
  return `${storage.name} (${storage.has_shell ? 'files and a shell' : 'files only'})`;
}
