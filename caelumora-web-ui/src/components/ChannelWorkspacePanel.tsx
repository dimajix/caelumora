// The session's workspace as the channel shows it (issues 251, 253): its
// name, classification and what it holds, and its files as its access list
// lets the user (issue 23) - or that there is none here, without why: the
// channel does not use it, the user may not read it, or none is attached.

import React, {useEffect, useMemo, useState} from 'react';
import {HardDrive, Shield} from 'lucide-react';
import type {WorkspaceResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {classificationDescription, classificationLabel} from '../confidentiality';
import {FileBrowser} from './FileBrowser';

interface ChannelWorkspacePanelProps {
  /** The session's workspace as the channel names it, if the user may read it. */
  workspaceId?: string;
  /** Changes whenever the workspace or its files may have changed - a tool call ended, the channel's switch. */
  version: number;
}

export const ChannelWorkspacePanel: React.FC<ChannelWorkspacePanelProps> = ({workspaceId, version}) => {
  // What was fetched, and for which workspace: `null` where it could not be.
  const [fetched, setFetched] = useState<{id: string; workspace: WorkspaceResponse | null} | null>(null);
  const files = useMemo(() => workspaceId ? api.workspaceFiles(workspaceId) : null, [workspaceId]);

  useEffect(() => {
    if (!workspaceId) return;
    let current = true;
    api.getWorkspace(workspaceId)
      .then(found => { if (current) setFetched({id: workspaceId, workspace: found}); })
      .catch(error => {
        if (!current) return;
        reportError('Failed to fetch the workspace of the channel', error);
        setFetched({id: workspaceId, workspace: null});
      });
    return () => { current = false; };
  }, [workspaceId, version]);

  // `undefined` while it is fetched, `null` where there is none to use.
  const workspace = !workspaceId ? null : fetched?.id === workspaceId ? fetched.workspace : undefined;

  if (workspace === undefined) return null;
  if (workspace === null || !files) {
    return <div className="text-xs italic text-gray-500">No workspace is used in this channel.</div>;
  }
  return (
    <div className="space-y-3">
      <div className="space-y-1">
        <div className="flex items-center gap-2 text-sm font-medium text-gray-800 dark:text-gray-100">
          <HardDrive size={14} className="shrink-0 text-gray-400" />
          <span className="truncate">{workspace.name}</span>
        </div>
        {workspace.description && <div className="text-xs text-gray-500 dark:text-gray-400">{workspace.description}</div>}
        <div className="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400" title={classificationDescription(workspace.classification)}>
          <Shield size={12} /> {classificationLabel(workspace.classification)}
        </div>
      </div>
      {!workspace.can_write && <div className="text-xs italic text-gray-500">You may read its files, not change them.</div>}
      <FileBrowser files={files} version={version} canWrite={workspace.can_write} />
    </div>
  );
};
