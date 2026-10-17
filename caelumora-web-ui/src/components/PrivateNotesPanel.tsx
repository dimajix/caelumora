// What an agent keeps to itself (issue 233): its private notes in every
// channel of the session the user may read, by the channel they were
// written in - this channel first, the others by name - for whoever may
// read what it saw. Numbers only for this channel's, as the agent is shown
// them: a number names a note in one channel. Fetched again whenever the
// agent's state changes anywhere in the session.

import React, {useEffect, useState} from 'react';
import {Lock, X} from 'lucide-react';
import type {ChannelParticipant, PrivateNotes} from '../types';
import {api} from '../api';
import {actorUuid} from '../mention';
import {ApiRequestError, reportError} from '../errors';
import {changedAgentState, onGlobalEvent} from '../globalEvents';
import {classificationDescription, classificationLabel} from '../confidentiality';
import {MessageTime} from './MessageView';

interface PrivateNotesPanelProps {
  sessionId: string;
  channelId: string;
  /** The agent whose private notes are shown. */
  participant: ChannelParticipant;
  onClose: () => void;
}

export const PrivateNotesPanel: React.FC<PrivateNotesPanelProps> = ({sessionId, channelId, participant, onClose}) => {
  const agent = actorUuid(participant.id);
  const [found, setFound] = useState<PrivateNotes[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!agent) return;
    let current = true;
    const load = () => api.getPrivateNotes(sessionId, agent)
      .then(response => { if (current) setFound(response); })
      .catch(error => {
        if (error instanceof ApiRequestError && error.code === 'context_forbidden') {
          if (current) setNotice('You may not see what this agent keeps to itself.');
        } else {
          reportError('Failed to load the private notes', error);
        }
      });
    void load();
    const unsubscribe = onGlobalEvent(event => {
      const changed = changedAgentState(event);
      if (changed?.sessionId === sessionId && changed.agentId === agent) void load();
    });
    return () => { current = false; unsubscribe(); };
  }, [sessionId, agent]);

  // This channel's first, the others as the server sorted them, by name.
  const groups = found && [
    ...found.filter(group => group.channel_id === channelId),
    ...found.filter(group => group.channel_id !== channelId),
  ];

  return (
    <div className="w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex items-center gap-2 text-gray-700 dark:text-gray-200">
          <Lock size={18} />
          <h3 className="font-semibold text-sm flex-1">The private notes of {participant.name}</h3>
          <button onClick={onClose} className="p-1 hover:bg-gray-100 dark:hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="text-xs text-gray-500 dark:text-gray-400">
          What the agent noted for itself in the channels of this session you can read. No other participant sees
          them; the agent recalls them wherever their classification allows.
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {notice && <div className="text-sm text-gray-500 dark:text-gray-400">{notice}</div>}
        {groups === null && !notice && <div className="text-sm text-gray-500 dark:text-gray-400">Loading…</div>}
        {groups?.length === 0 && <div className="text-sm text-gray-500 dark:text-gray-400">Nothing noted privately.</div>}
        {groups?.map(group => {
          const here = group.channel_id === channelId;
          return (
            <div key={group.channel_id} className="space-y-2">
              <div className="text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">
                {here ? `This channel (${group.channel_name})` : group.channel_name}
              </div>
              {group.notes.map(note => (
                <div key={note.id} className="rounded-md border border-gray-200 dark:border-gray-700 p-2 text-sm text-gray-800 dark:text-gray-200">
                  <div className="whitespace-pre-wrap">{note.text}</div>
                  <div className="mt-1 flex items-center gap-2 text-[11px] text-gray-500 dark:text-gray-400">
                    {here && <span className="font-mono">#{note.id}</span>}
                    <MessageTime timestamp={note.written_at} />
                    <span title={classificationDescription(note.classification)}>{classificationLabel(note.classification)}</span>
                  </div>
                </div>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
};
