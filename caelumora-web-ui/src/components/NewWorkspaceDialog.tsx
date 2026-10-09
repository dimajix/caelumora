// Making a workspace (issue 253): its name, the storage its files lie in -
// and what kind of workspace that makes (issue 266) -, where it lies - the
// user's scope or the global one; one of a session's scope is made with the
// session - what it holds, and its classification, a level and compartments
// (issue 133).

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import type {StorageResponse, WorkspaceResponse} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {DEFAULT_CLASSIFICATION, type Classification} from '../confidentiality';
import {storageLabel} from '../workspace';
import {WorkspaceClassificationFields} from './WorkspaceClassificationFields';

interface NewWorkspaceDialogProps {
  onCreated: (workspace: WorkspaceResponse) => void;
  onClose: () => void;
}

export const NewWorkspaceDialog: React.FC<NewWorkspaceDialogProps> = ({onCreated, onClose}) => {
  const [name, setName] = useState('');
  const [storages, setStorages] = useState<StorageResponse[] | null>(null);
  const [storage, setStorage] = useState('');
  const [scope, setScope] = useState<'user' | 'global'>('user');
  const [description, setDescription] = useState('');
  const [classification, setClassification] = useState<Classification>(DEFAULT_CLASSIFICATION);
  const [problem, setProblem] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  useEffect(() => {
    api.getStorages()
      .then(found => {
        setStorages(found);
        setStorage(current => current || (found[0]?.name ?? ''));
      })
      .catch(error => {
        reportError('Failed to fetch the storages', error);
        setStorages([]);
      });
  }, []);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || !storage || isCreating) return;
    setIsCreating(true);
    setProblem(null);
    try {
      const workspace = await api.createWorkspace({
        name: name.trim(),
        scope,
        storage,
        description: description.trim() || undefined,
        classification,
      });
      onCreated(workspace);
    } catch (error) {
      // What the server says about the request - a taken name, a
      // classification the user is not cleared for - is shown here.
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError('Failed to create the workspace', error);
      }
    } finally {
      setIsCreating(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
      onKeyDown={(e) => { if (e.key === 'Escape') onClose(); }}
    >
      <form
        onSubmit={submit}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-workspace-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="new-workspace-title" className="flex-1 font-semibold text-sm">New workspace</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name</span>
            <input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1 focus:outline-hidden focus:ring-2 focus:ring-blue-500"
            />
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Storage</span>
            <select
              value={storage}
              onChange={(e) => setStorage(e.target.value)}
              disabled={!storages || storages.length === 0}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
            >
              {(storages ?? []).map(s => <option key={s.name} value={s.name}>{storageLabel(s)}</option>)}
            </select>
            {storages?.length === 0 && (
              <span className="block text-xs text-red-400">No storage is configured: no workspace can be made.</span>
            )}
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name space</span>
            <select
              value={scope}
              onChange={(e) => setScope(e.target.value as 'user' | 'global')}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
            >
              <option value="user">Yours</option>
              <option value="global">Global</option>
            </select>
            <span className="block text-xs text-gray-500">
              {scope === 'user'
                ? 'Its name is unique among your workspaces.'
                : 'Its name is unique among all workspaces. It is yours all the same.'}
            </span>
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Description</span>
            <input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What it holds"
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1 focus:outline-hidden focus:ring-2 focus:ring-blue-500"
            />
          </label>

          <WorkspaceClassificationFields value={classification} onChange={setClassification} />

          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={!name.trim() || !storage || isCreating}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isCreating ? 'Creating…' : 'Create'}
          </button>
        </div>
      </form>
    </div>
  );
};
