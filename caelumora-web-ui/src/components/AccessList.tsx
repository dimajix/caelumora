// Whom a workspace or an artifact is shared with (issue 23), for its owner:
// the entries of its access list, each with what it grants.

import React from 'react';
import {Trash2} from 'lucide-react';
import type {AccessEntry, Grant} from '../types';

/** What `kind` of whom an entry names is called, for a reader. */
const KIND_LABELS: Record<AccessEntry['kind'], string> = {
  user: 'User',
  role: 'Role',
  session: 'Members of the session',
  channel: 'Members of the channel',
};

interface AccessListProps {
  access: AccessEntry[];
  onChange: (access: AccessEntry[]) => void;
  /** What is said where it is shared with nobody. */
  nobody: string;
}

/**
 * Whom something is shared with (issue 23), for its owner: each entry with
 * what it grants, which can be changed, or taken away.
 */
export const AccessList: React.FC<AccessListProps> = ({access, onChange, nobody}) => (
  <div className="space-y-1">
    <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Shared with</h3>
    {access.length === 0 ? (
      <div className="text-xs italic text-gray-500">{nobody}</div>
    ) : (
      <ul className="space-y-1 text-sm">
        {access.map((entry, index) => (
          <li key={`${entry.kind}-${entry.id}`} className="flex items-center gap-2">
            <span className="flex-1 min-w-0 truncate">
              <span className="text-xs text-gray-500">{KIND_LABELS[entry.kind]}: </span>
              {entry.name ?? <span className="italic text-gray-500">gone</span>}
            </span>
            <select
              value={entry.grant}
              onChange={(e) => onChange(access.map((other, i) => i === index ? {...other, grant: e.target.value as Grant} : other))}
              className="rounded-md border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-1 py-0.5 text-xs"
              aria-label={`What ${entry.name ?? 'it'} may do`}
            >
              <option value="read">reads</option>
              <option value="write">reads and changes</option>
            </select>
            <button
              type="button"
              onClick={() => onChange(access.filter((_, i) => i !== index))}
              className="p-1 rounded-md text-gray-500 hover:text-red-500 hover:bg-gray-100 dark:hover:bg-gray-800"
              title="No longer share with them"
              aria-label={`No longer share with ${entry.name ?? 'them'}`}
            >
              <Trash2 size={12} />
            </button>
          </li>
        ))}
      </ul>
    )}
  </div>
);
