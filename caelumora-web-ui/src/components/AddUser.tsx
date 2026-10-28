// Picking a user to add to the members of a team (issue 286) or a project (issue 467), from the
// participants of a session the user may read: the REST API lists no users.

import React, {useEffect, useState} from 'react';
import {Plus} from 'lucide-react';
import type {SessionParticipant, SessionResponse} from '../types';
import {userIdOf} from '../actor';
import {api} from '../api';
import {reportError} from '../errors';

interface AddUserProps {
  /** The users among the members already, by id: they are not offered. */
  members: string[];
  onAdd: (userId: string) => void;
}

/**
 * The session chosen first, its participants loaded once it is, as `AccessList` picks the members
 * of a session.
 */
export const AddUser: React.FC<AddUserProps> = ({members, onAdd}) => {
  const [sessions, setSessions] = useState<SessionResponse[]>([]);
  const [sessionId, setSessionId] = useState('');
  const [participants, setParticipants] = useState<SessionParticipant[]>([]);
  const [chosen, setChosen] = useState('');

  useEffect(() => {
    let current = true;
    api.getSessions()
      .then(found => { if (current) setSessions(found); })
      .catch(error => reportError('Failed to fetch the sessions', error));
    return () => { current = false; };
  }, []);

  useEffect(() => {
    if (!sessionId) return;
    let current = true;
    api.getSession(sessionId)
      .then(found => { if (current) setParticipants(found.participants ?? []); })
      .catch(error => reportError('Failed to load the participants of the session', error));
    return () => { current = false; };
  }, [sessionId]);

  const choose = (session: string) => {
    setParticipants([]);
    setChosen('');
    setSessionId(session);
  };

  // The users of the session who are not among the members yet.
  const candidates = participants
    .flatMap(p => {
      const userId = p.kind === 'user' ? userIdOf(p.id) : null;
      return userId && !members.includes(userId) ? [{userId, name: p.name}] : [];
    })
    .sort((l, r) => l.name.localeCompare(r.name));

  const add = () => {
    if (!chosen) return;
    onAdd(chosen);
    setChosen('');
  };

  if (sessions.length === 0) return null;

  const field = "rounded-md border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-1 py-0.5 text-xs";
  return (
    <div className="flex flex-wrap items-center gap-2 pt-1 text-xs">
      <span className="text-gray-500">Add someone from</span>
      <select value={sessionId} onChange={(e) => choose(e.target.value)} className={field} aria-label="The session">
        <option value="">a session …</option>
        {sessions.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}
      </select>
      {sessionId && (
        candidates.length === 0 ? (
          <span className="italic text-gray-500">Nobody else to add there</span>
        ) : (
          <>
            <select value={chosen} onChange={(e) => setChosen(e.target.value)} className={field} aria-label="The user">
              <option value="">a user …</option>
              {candidates.map(c => <option key={c.userId} value={c.userId}>{c.name}</option>)}
            </select>
            <button
              type="button"
              onClick={add}
              disabled={!chosen}
              className="flex items-center gap-1 rounded-md border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800 px-2 py-0.5 disabled:opacity-50"
            >
              <Plus size={12} /> Add
            </button>
          </>
        )
      )}
    </div>
  );
};
