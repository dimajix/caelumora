// How workspaces are shown (issue 253): labels of what the server says.

import type {StorageResponse} from './types';

/** A storage as people choose it: its name and the kind of workspace it makes (issue 266). */
export function storageLabel(storage: StorageResponse): string {
  return `${storage.name} (${storage.has_shell ? 'files and a shell' : 'files only'})`;
}
