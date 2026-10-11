// One team (issue 286), in the place of a channel: its name, where it lies and who is in it. Its
// owner renames and deletes it from its menu, takes members out and adds users met in a session
// they may read - every change writes the whole list of members. A member who does not own it
// sees who is in it and changes nothing. The participants of a session or a channel are a group
// too, read here and changed only by joining and leaving.

import React, {useEffect, useState} from 'react';
import {Edit2, Plus, Trash2, UsersRound} from 'lucide-react';
import type {ActorId, GroupResponse, SessionParticipant, SessionResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {actorKey} from '../mention';
import {ActionMenu} from './ActionMenu';
import {scopeLabel} from '../scope';

/** The user `id` names, or `null` for an actor that is no user. */
function userIdOf(id: ActorId): string | null {
  return typeof id !== 'string' && typeof id.User === 'string' ? id.User : null;
}

interface GroupPageProps {
  groupId: string;
  onDeleted: (groupId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const GroupPage: React.FC<GroupPageProps> = ({groupId, onDeleted}) => {
  const [group, setGroup] = useState<GroupResponse | null>(null);
  const [isRenaming, setIsRenaming] = useState(false);
  const [name, setName] = useState('');

  useEffect(() => {
    let current = true;
    api.getGroup(groupId)
      .then(found => { if (current) setGroup(found); })
      .catch(error => reportError('Failed to load the team', error));
    return () => { current = false; };
  }, [groupId]);

  if (!group) return null;

  // The users in the team, by id - what a change of its members writes anew.
  const memberIds = group.members.flatMap(m => {
    const userId = userIdOf(m.id);
    return userId ? [userId] : [];
  });

  const rename = async () => {
    setIsRenaming(false);
    if (!name.trim() || name.trim() === group.name) return;
    try {
      setGroup(await api.renameGroup(group.id, name.trim()));
    } catch (error) {
      reportError('Failed to rename the team', error);
    }
  };

  const setMembers = async (userIds: string[]) => {
    try {
      setGroup(await api.setGroupMembers(group.id, userIds));
    } catch (error) {
      reportError('Failed to change the members of the team', error);
    }
  };

  const remove = async () => {
    try {
      await api.deleteGroup(group.id);
      onDeleted(group.id);
    } catch (error) {
      reportError('Failed to delete the team', error);
    }
  };

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <div className="flex items-start gap-3">
          <UsersRound size={22} className="mt-1 shrink-0 text-gray-400" />
          <div className="flex-1 min-w-0 space-y-1">
            {isRenaming ? (
              <input
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                onBlur={() => void rename()}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void rename();
                  if (e.key === 'Escape') setIsRenaming(false);
                }}
                className="w-full rounded-md border border-blue-500 bg-white dark:bg-gray-800 px-2 py-1 text-sm"
              />
            ) : (
              <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{group.name}</h2>
            )}
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
              <span>Name space: {scopeLabel(group)}</span>
            </div>
          </div>
          {group.can_change && (
            <ActionMenu
              label={`Options of ${group.name}`}
              size={16}
              className="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
              actions={[
                {label: 'Rename', icon: Edit2, onSelect: () => { setName(group.name); setIsRenaming(true); }},
                {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete team?', onSelect: () => void remove()},
              ]}
            />
          )}
        </div>

        <div className="text-sm text-gray-600 dark:text-gray-400">
          {group.scope === 'session'
            ? `The ${group.channel_id ? 'participants of a channel' : 'participants of a session'}: who is in it changes as they join and leave.`
            : group.can_change
              ? 'What you share with the team, everyone in it reads, as long as they are in it.'
              : 'You are in this team. Its owner chooses who is in it.'}
        </div>

        <div className="space-y-1">
          <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Members</h3>
          {group.members.length === 0 ? (
            <div className="text-xs italic text-gray-500">Nobody is in it yet.</div>
          ) : (
            <ul className="space-y-1 text-sm">
              {group.members.map(member => {
                const userId = userIdOf(member.id);
                return (
                  <li key={actorKey(member.id)} className="flex items-center gap-2">
                    <span className="flex-1 min-w-0 truncate">
                      {member.name ?? <span className="italic text-gray-500">unknown</span>}
                    </span>
                    {group.can_change && userId && (
                      <button
                        type="button"
                        onClick={() => void setMembers(memberIds.filter(id => id !== userId))}
                        className="p-1 rounded-md text-gray-500 hover:text-red-500 hover:bg-gray-100 dark:hover:bg-gray-800"
                        title="Take them out of the team"
                        aria-label={`Take ${member.name ?? 'them'} out of the team`}
                      >
                        <Trash2 size={12} />
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {group.can_change && (
            <AddMember members={memberIds} onAdd={(userId) => void setMembers([...memberIds, userId])} />
          )}
        </div>
      </div>
    </div>
  );
};

interface AddMemberProps {
  /** The users in the team already, by id: they are not offered. */
  members: string[];
  onAdd: (userId: string) => void;
}

/**
 * Picks a user to add to the team: the REST API lists no users, so they are
 * those taking part in a session the user may read - the session chosen
 * first, its participants loaded once it is, as `AccessList` picks the
 * members of a session.
 */
const AddMember: React.FC<AddMemberProps> = ({members, onAdd}) => {
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

  // The users of the session who are not in the team yet.
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
