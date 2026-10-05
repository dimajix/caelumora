// What was deleted - sessions, or the channels of a session - folded below
// what is there (issue 164): who deleted it and when, with "Restore" and
// "Delete for good" where the server says the user may. Loaded when
// shown, and again whenever `version` changes; shown only while it holds
// an entry the user may see (issue 232) - the server leaves out the others.

import React, {useEffect, useState} from 'react';
import {ChevronDown, ChevronRight, RotateCcw, Trash2} from 'lucide-react';
import type {Deleted} from '../types';
import {reportError} from '../errors';
import {formatTimestamp} from '../messages';

interface DeletedListProps {
  /** The heading, e.g. "Deleted sessions". */
  title: string;
  load: () => Promise<Deleted[]>;
  /** Changes whenever something may have been deleted or restored. */
  version: number;
  onRestore: (id: string) => Promise<void>;
  /** Removes it for good; offered only where `can_purge`. */
  onPurge?: (id: string) => Promise<void>;
}

export const DeletedList: React.FC<DeletedListProps> = ({title, load, version, onRestore, onPurge}) => {
  const [open, setOpen] = useState(false);
  const [deleted, setDeleted] = useState<Deleted[] | null>(null);
  // The entry whose "Delete for good" was clicked once, to be confirmed.
  const [confirming, setConfirming] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    load()
      .then(found => { if (current) setDeleted(found); })
      .catch(error => reportError(`Failed to load the ${title.toLowerCase()}`, error));
    return () => { current = false; };
    // `load` is a new function on every render; what it loads changes with `version`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version]);

  const act = async (action: (id: string) => Promise<void>, id: string) => {
    setConfirming(null);
    await action(id);
    setDeleted(prev => prev && prev.filter(d => d.id !== id));
  };

  // Nothing deleted, nothing the user may see, or not loaded (yet): no heading.
  if (!deleted || deleted.length === 0) return null;

  return (
    <div>
      <button
        onClick={() => setOpen(shown => !shown)}
        className="w-full flex items-center p-2 rounded-md hover:bg-gray-800 transition-colors text-xs text-gray-500"
        aria-expanded={open}
      >
        {open ? <ChevronDown size={14} className="mr-2 shrink-0" /> : <ChevronRight size={14} className="mr-2 shrink-0" />}
        {title}
      </button>
      {open && (
        <div className="ml-4 space-y-1">
          {deleted.map(entry => {
            const at = formatTimestamp(entry.deleted.at);
            return (
              <div key={entry.id} className="group/deleted flex items-center rounded-md hover:bg-gray-800 text-xs text-gray-500">
                <div className="flex-1 min-w-0 p-2">
                  <div className="truncate line-through">{entry.name}</div>
                  {at && <div className="text-[11px]" title={at.full}>deleted {at.short}</div>}
                </div>
                {entry.can_restore && (
                  <button
                    onClick={() => void act(onRestore, entry.id).catch(error => reportError(`Failed to restore ${entry.name}`, error))}
                    className="p-1 rounded-md hover:text-white hover:bg-gray-700"
                    title={`Restore ${entry.name}`}
                    aria-label={`Restore ${entry.name}`}
                  >
                    <RotateCcw size={12} />
                  </button>
                )}
                {entry.can_purge && onPurge && (confirming === entry.id ? (
                  <button
                    onClick={() => void act(onPurge, entry.id).catch(error => reportError(`Failed to delete ${entry.name} for good`, error))}
                    className="px-1 py-0.5 mr-1 rounded-md text-red-400 hover:bg-gray-700"
                  >
                    Delete for good?
                  </button>
                ) : (
                  <button
                    onClick={() => setConfirming(entry.id)}
                    className="p-1 mr-1 rounded-md text-red-400 hover:bg-gray-700"
                    title={`Delete ${entry.name} for good`}
                    aria-label={`Delete ${entry.name} for good`}
                  >
                    <Trash2 size={12} />
                  </button>
                ))}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
