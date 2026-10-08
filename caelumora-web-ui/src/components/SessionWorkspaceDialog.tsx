// The workspace of a session (issue 253): the one attached, as far as the
// user may read it, and attaching one of theirs - not one made for another
// session - or taking it away. Who may is the server's to decide: attaching
// is the workspace's owner's, into a session they may change.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import type {SessionSnapshot, WorkspaceResponse} from '../types';
import {ApiRequestError, reportError} from '../errors';
import {workspaceScopeLabel} from '../workspace';

interface SessionWorkspaceDialogProps {
  session: SessionSnapshot;
  /** The user's own workspaces. */
  workspaces: WorkspaceResponse[];
  onSubmit: (workspaceId: string | null) => Promise<void>;
  onClose: () => void;
}

export const SessionWorkspaceDialog: React.FC<SessionWorkspaceDialogProps> = ({session, workspaces, onSubmit, onClose}) => {
  // One made for another session is used there alone.
  const offered = workspaces.filter(w => w.scope !== 'session' || w.session_id === session.id);
  const [chosen, setChosen] = useState(session.workspace?.id ?? offered[0]?.id ?? '');
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  const act = async (workspaceId: string | null) => {
    setIsSaving(true);
    setProblem(null);
    try {
      await onSubmit(workspaceId);
      onClose();
    } catch (error) {
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError(workspaceId === null ? 'Failed to detach the workspace' : 'Failed to attach the workspace', error);
      }
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
      onKeyDown={(e) => { if (e.key === 'Escape') onClose(); }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="session-workspace-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="session-workspace-title" className="flex-1 font-semibold text-sm truncate">Workspace of {session.name}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <div className="text-xs text-gray-400">
            Its channels share it: their members work on its files where the channel uses it and is classified
            alike.
          </div>
          <div>
            <span className="text-xs text-gray-400">Attached: </span>
            {session.workspace
              ? <span>{session.workspace.name} <span className="text-xs text-gray-500">({workspaceScopeLabel(session.workspace)})</span></span>
              : <span className="italic text-gray-500">none of yours</span>}
          </div>

          {session.can_change && offered.length > 0 && (
            <label className="block space-y-1">
              <span className="text-xs text-gray-400">Attach</span>
              <select
                value={chosen}
                onChange={(e) => setChosen(e.target.value)}
                className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
              >
                {offered.map(w => <option key={w.id} value={w.id}>{w.name} ({workspaceScopeLabel(w)})</option>)}
              </select>
            </label>
          )}
          {session.can_change && offered.length === 0 && (
            <div className="text-xs italic text-gray-500">You have no workspace to attach.</div>
          )}
          {!session.can_change && (
            <div className="text-xs italic text-gray-500">Only who may change the session attaches a workspace.</div>
          )}

          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          {session.can_change && session.workspace && (
            <button type="button" onClick={() => void act(null)} disabled={isSaving} className="mr-auto rounded-md px-3 py-1 text-sm text-red-400 hover:bg-gray-800 disabled:opacity-50">
              Detach
            </button>
          )}
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Close
          </button>
          {session.can_change && offered.length > 0 && (
            <button
              type="button"
              onClick={() => void act(chosen)}
              disabled={!chosen || isSaving || chosen === session.workspace?.id}
              className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
            >
              Attach
            </button>
          )}
        </div>
      </div>
    </div>
  );
};
