// The workspace of a session (issue 253): the one attached, as far as the
// user may read it, and attaching one of theirs - not one made for another
// session -, shared with the session's members as far as chosen (issue 23),
// or taking it away. Who may is the server's to decide: attaching is the
// workspace's owner's, into a session they may change.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import type {SessionSnapshot, WorkspaceResponse} from '../types';
import {reportUnlessRefused} from '../errors';
import {scopeLabel} from '../scope';
import {ShareSelect, type Share} from './ShareSelect';
import {Refusal} from './Refusal';

interface SessionWorkspaceDialogProps {
  session: SessionSnapshot;
  /** The user's own workspaces. */
  workspaces: WorkspaceResponse[];
  /** The user choosing: whether what is offered is theirs. */
  userId: string;
  onSubmit: (workspaceId: string | null, share: Share) => Promise<void>;
  onClose: () => void;
}

export const SessionWorkspaceDialog: React.FC<SessionWorkspaceDialogProps> = ({session, workspaces, userId, onSubmit, onClose}) => {
  // Only one's own is attached; one made for another session is used there alone.
  const offered = workspaces.filter(w => w.can_change && (w.scope !== 'session' || w.session_id === session.id));
  const [chosen, setChosen] = useState(session.workspace?.id ?? offered[0]?.id ?? '');
  const [share, setShare] = useState<Share>('write');
  const [refusal, setRefusal] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  const act = async (workspaceId: string | null) => {
    setIsSaving(true);
    setRefusal(null);
    try {
      await onSubmit(workspaceId, share);
      onClose();
    } catch (error) {
      reportUnlessRefused(workspaceId === null ? 'Failed to detach the workspace' : 'Failed to attach the workspace', error, setRefusal);
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
            Its channels use it where they are classified alike - for those its access list lets: attaching it
            shares it with the session's members only as chosen below.
          </div>
          <div>
            <span className="text-xs text-gray-400">Attached: </span>
            {session.workspace
              ? <span>{session.workspace.name} <span className="text-xs text-gray-500">({scopeLabel(session.workspace, userId)})</span></span>
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
                {offered.map(w => <option key={w.id} value={w.id}>{w.name} ({scopeLabel(w, userId)})</option>)}
              </select>
            </label>
          )}
          {session.can_change && offered.length > 0 && <ShareSelect value={share} onChange={setShare} />}
          {session.can_change && offered.length === 0 && (
            <div className="text-xs italic text-gray-500">You have no workspace to attach.</div>
          )}
          {!session.can_change && (
            <div className="text-xs italic text-gray-500">Only who may change the session attaches a workspace.</div>
          )}

          <Refusal message={refusal} />
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
              disabled={!chosen || isSaving || (chosen === session.workspace?.id && share === null)}
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
