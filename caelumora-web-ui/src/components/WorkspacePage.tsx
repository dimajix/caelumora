// One of the user's workspaces (issue 253), in the place of a channel: what
// it is - its name, where it lies, its storage, its classification and what
// it holds - with what its owner may do to it, and its files. One of a
// session's scope is used within that session alone: its files are reached
// from a channel of it, not here.

import React, {useEffect, useMemo, useState} from 'react';
import {Edit2, FileText, HardDrive, Shield, Trash2} from 'lucide-react';
import type {Compartment, WorkspaceResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {CONFIDENTIALITY_LEVELS, classificationDescription, classificationLabel} from '../confidentiality';
import {ActionMenu} from './ActionMenu';
import {ClassificationDialog} from './ClassificationDialog';
import {FileBrowser} from './FileBrowser';
import {workspaceScopeLabel} from '../workspace';

interface WorkspacePageProps {
  workspaceId: string;
  onDeleted: (workspaceId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const WorkspacePage: React.FC<WorkspacePageProps> = ({workspaceId, onDeleted}) => {
  const [workspace, setWorkspace] = useState<WorkspaceResponse | null>(null);
  const [editing, setEditing] = useState<'name' | 'description' | null>(null);
  const [text, setText] = useState('');
  const [isReclassifying, setIsReclassifying] = useState(false);
  const [compartments, setCompartments] = useState<Compartment[]>([]);
  const files = useMemo(() => api.ownedFiles(workspaceId), [workspaceId]);

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

  const changed = (updated: WorkspaceResponse) => setWorkspace(updated);

  const edit = (field: 'name' | 'description') => {
    setText(field === 'name' ? workspace.name : (workspace.description ?? ''));
    setEditing(field);
  };

  const save = async () => {
    const field = editing;
    setEditing(null);
    try {
      if (field === 'name' && text.trim() && text.trim() !== workspace.name) {
        changed(await api.renameWorkspace(workspace.id, text.trim()));
      } else if (field === 'description' && text.trim() !== (workspace.description ?? '')) {
        changed(await api.describeWorkspace(workspace.id, text.trim() || null));
      }
    } catch (error) {
      reportError(field === 'name' ? 'Failed to rename the workspace' : 'Failed to describe the workspace', error);
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

  const field = (
    <input
      autoFocus
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => void save()}
      onKeyDown={(e) => {
        if (e.key === 'Enter') void save();
        if (e.key === 'Escape') setEditing(null);
      }}
      className="w-full rounded-md border border-blue-500 bg-white dark:bg-gray-800 px-2 py-1 text-sm"
    />
  );

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <div className="flex items-start gap-3">
          <HardDrive size={22} className="mt-1 shrink-0 text-gray-400" />
          <div className="flex-1 min-w-0 space-y-1">
            {editing === 'name' ? field : <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{workspace.name}</h2>}
            {editing === 'description' ? field : (
              <p className="text-sm text-gray-600 dark:text-gray-400">{workspace.description || <span className="italic">No description</span>}</p>
            )}
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
              <span>Name space: {workspaceScopeLabel(workspace)}</span>
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
                {label: 'Rename', icon: Edit2, onSelect: () => edit('name')},
                {label: 'Description', icon: FileText, onSelect: () => edit('description')},
                {label: 'Classification', icon: Shield, onSelect: () => setIsReclassifying(true)},
                {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete workspace?', onSelect: () => void remove()},
              ]}
            />
          )}
        </div>

        {workspace.scope === 'session' ? (
          <div className="text-sm text-gray-600 dark:text-gray-400">
            It was made for one session and is used within it alone: its files are reached from a channel of that session.
          </div>
        ) : (
          <FileBrowser files={files} />
        )}
      </div>

      {isReclassifying && (
        <ClassificationDialog
          title={`Classification of ${workspace.name}`}
          current={workspace.classification}
          levels={CONFIDENTIALITY_LEVELS}
          compartments={compartments}
          releaseWarning="Every file in it is released to a wider circle. Which channels may use it follows."
          onSubmit={async (classification) => changed(await api.reclassifyWorkspace(workspace.id, classification))}
          onClose={() => setIsReclassifying(false)}
        />
      )}
    </div>
  );
};
