// A workspace the user may read (issue 253), in the place of a channel: what
// it is - its name, where it lies, its storage, its classification and what
// it holds - with what its owner may do to it, whom it is shared with (issue
// 23), and its files, as its access list lets the user.

import React, {useEffect, useMemo, useState} from 'react';
import {Edit2, FileText, HardDrive, Shield, Trash2} from 'lucide-react';
import type {AccessEntry, Compartment, WorkspaceResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {CONFIDENTIALITY_LEVELS, classificationDescription, classificationLabel} from '../confidentiality';
import {AccessList} from './AccessList';
import {ActionMenu} from './ActionMenu';
import {ClassificationDialog} from './ClassificationDialog';
import {FileBrowser} from './FileBrowser';
import {scopeLabel} from '../scope';
import {EditInPlace} from './EditInPlace';

interface WorkspacePageProps {
  workspaceId: string;
  onDeleted: (workspaceId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const WorkspacePage: React.FC<WorkspacePageProps> = ({workspaceId, onDeleted}) => {
  const [workspace, setWorkspace] = useState<WorkspaceResponse | null>(null);
  const [editing, setEditing] = useState<'name' | 'description' | null>(null);
  const [isReclassifying, setIsReclassifying] = useState(false);
  const [compartments, setCompartments] = useState<Compartment[]>([]);
  const files = useMemo(() => api.workspaceFiles(workspaceId), [workspaceId]);

  useEffect(() => {
    let current = true;
    api.getWorkspace(workspaceId)
      .then(found => { if (current) setWorkspace(found); })
      .catch(error => reportError('Failed to load the workspace', error));
    api.getCompartments()
      .then(found => { if (current) setCompartments(found); })
      .catch(error => reportError('Failed to fetch the compartments', error));
    return () => { current = false; };
  }, [workspaceId]);

  if (!workspace) return null;

  const rename = async (name: string) => {
    if (name) setWorkspace(await api.renameWorkspace(workspace.id, name));
  };

  const describe = async (description: string) => {
    setWorkspace(await api.describeWorkspace(workspace.id, description || null));
  };

  const share = async (access: AccessEntry[]) => {
    try {
      setWorkspace(await api.setWorkspaceAccess(workspace.id, access));
    } catch (error) {
      reportError('Failed to share the workspace', error);
    }
  };

  const remove = async () => {
    try {
      await api.deleteWorkspace(workspace.id);
      onDeleted(workspace.id);
    } catch (error) {
      reportError('Failed to delete the workspace', error);
    }
  };

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <div className="flex items-start gap-3">
          <HardDrive size={22} className="mt-1 shrink-0 text-gray-400" />
          <div className="flex-1 min-w-0 space-y-1">
            {editing === 'name' ? <EditInPlace initial={workspace.name} save={rename} action="Failed to rename the workspace" onDone={() => setEditing(null)} /> : <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{workspace.name}</h2>}
            {editing === 'description' ? <EditInPlace initial={workspace.description ?? ''} save={describe} action="Failed to describe the workspace" onDone={() => setEditing(null)} /> : (
              <p className="text-sm text-gray-600 dark:text-gray-400">{workspace.description || <span className="italic">No description</span>}</p>
            )}
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
              <span>Name space: {scopeLabel(workspace)}</span>
              <span>Storage: {workspace.storage}</span>
              <span className="flex items-center gap-1" title={classificationDescription(workspace.classification)}>
                <Shield size={12} /> {classificationLabel(workspace.classification)}
              </span>
            </div>
          </div>
          {workspace.can_change && (
            <ActionMenu
              label={`Options of ${workspace.name}`}
              size={16}
              className="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
              actions={[
                {label: 'Rename', icon: Edit2, onSelect: () => setEditing('name')},
                {label: 'Description', icon: FileText, onSelect: () => setEditing('description')},
                {label: 'Classification', icon: Shield, onSelect: () => setIsReclassifying(true)},
                {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete workspace?', onSelect: () => void remove()},
              ]}
            />
          )}
        </div>

        {workspace.can_change && (
          <AccessList
            access={workspace.access ?? []}
            onChange={(access) => void share(access)}
            nobody="Nobody but you."
            grants={{read: 'reads', write: 'reads and changes'}}
          />
        )}
        {!workspace.can_write && <div className="text-xs italic text-gray-500">You may read its files, not change them.</div>}
        <FileBrowser files={files} canWrite={workspace.can_write} />
      </div>

      {isReclassifying && (
        <ClassificationDialog
          title={`Classification of ${workspace.name}`}
          current={workspace.classification}
          levels={CONFIDENTIALITY_LEVELS}
          compartments={compartments}
          releaseWarning="Every file in it is released to a wider circle. Which channels may use it follows."
          onSubmit={async (classification) => setWorkspace(await api.reclassifyWorkspace(workspace.id, classification))}
          onClose={() => setIsReclassifying(false)}
        />
      )}
    </div>
  );
};
