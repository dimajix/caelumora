// The session's workspace as the user may use it in this channel (issues
// 251, 253): its name, classification and what it holds, and its files -
// or that there is none to use here, without why: the channel may not use
// it, the classifications may not let it, or none is attached.

import React, {useEffect, useMemo, useState} from 'react';
import {HardDrive, Shield} from 'lucide-react';
import type {WorkspaceResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {classificationDescription, classificationLabel} from '../confidentiality';
import {FileBrowser} from './FileBrowser';

interface ChannelWorkspacePanelProps {
  sessionId: string;
  channelId: string;
  /** Changes whenever the workspace or its files may have changed - a tool call ended, the channel's switch. */
  version: number;
}

export const ChannelWorkspacePanel: React.FC<ChannelWorkspacePanelProps> = ({sessionId, channelId, version}) => {
  // `undefined` while it is fetched, `null` where there is none to use.
  const [workspace, setWorkspace] = useState<WorkspaceResponse | null | undefined>(undefined);
  const files = useMemo(() => api.channelFiles(sessionId, channelId), [sessionId, channelId]);

  useEffect(() => {
    let current = true;
    api.getChannelWorkspace(sessionId, channelId)
      .then(found => { if (current) setWorkspace(found); })
      .catch(error => {
        if (!current) return;
        reportError('Failed to fetch the workspace of the channel', error);
        setWorkspace(null);
      });
    return () => { current = false; };
  }, [sessionId, channelId, version]);

  if (workspace === undefined) return null;
  if (workspace === null) {
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
      <FileBrowser files={files} version={version} />
    </div>
  );
};
