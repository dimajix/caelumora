// The notes the agents of a channel published (issue 17), read-only,
// grouped by agent.

import React from 'react';
import type {PublishedNotes} from '../types';
import {MessageTime} from './MessageView';

export const NotesList: React.FC<{notes: PublishedNotes[]}> = ({notes}) => {
  const groups = notes
    .filter(group => group.notes.length > 0)
    .sort((l, r) => l.name.localeCompare(r.name));
  if (groups.length === 0) {
    return <div className="text-sm text-gray-500 dark:text-gray-400">Nothing noted yet.</div>;
  }
  return (
    <div className="space-y-4">
      {groups.map(group => (
        <div key={JSON.stringify(group.agent_id)} className="space-y-2">
          <div className="text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">{group.name}</div>
          {group.notes.map(note => (
            <div key={note.id} className="rounded-md border border-gray-200 dark:border-gray-700 p-2 text-sm text-gray-800 dark:text-gray-200">
              <div className="whitespace-pre-wrap">{note.text}</div>
              <div className="mt-1 flex items-center gap-2 text-[11px] text-gray-500 dark:text-gray-400">
                <span className="font-mono">#{note.id}</span>
                <MessageTime timestamp={note.written_at} />
              </div>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
};
