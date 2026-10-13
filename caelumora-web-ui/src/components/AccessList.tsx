// Whom a workspace or an artifact is shared with (issue 23), for its owner:
// the entries of its access list, each with what it grants, and adding the
// members of a session or of one of its channels (issue 291) - picked from
// the sessions the user may read, a session's channels loaded once it is
// chosen - or a team the user may read (issue 286).

import React, {useEffect, useState} from 'react';
import {Plus, Trash2} from 'lucide-react';
import type {AccessEntry, ChannelResponse, Grant, GroupResponse, SessionResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';

/** What `kind` of whom an entry names is called, for a reader. */
const KIND_LABELS: Record<AccessEntry['kind'], string> = {
  user: 'User',
  role: 'Role',
  session: 'Members of the session',
  channel: 'Members of the channel',
  group: 'Team',
  everyone: 'Everyone',
};

/**
 * What each grant lets whom an entry names do, as a reader is told; joining (issue 221) only where
 * what is shared is a session or channel - elsewhere an entry that grants it is shown as it is.
 */
export type GrantLabels = Record<Exclude<Grant, 'join'>, string> & {join?: string};

interface AccessListProps {
  access: AccessEntry[];
  onChange: (access: AccessEntry[]) => void;
  /** What is said where it is shared with nobody. */
  nobody: string;
  /** What each grant means for what is shared - a workspace's `write` changes its files. */
  grants: GrantLabels;
}

/**
 * Whom something is shared with (issue 23), for its owner: each entry with
 * what it grants, which can be changed, or taken away, and below them the
 * members of a session or channel (issue 291) or a team (issue 286) to add.
 */
export const AccessList: React.FC<AccessListProps> = ({access, onChange, nobody, grants}) => (
  <div className="space-y-1">
    <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Shared with</h3>
    {access.length === 0 ? (
      <div className="text-xs italic text-gray-500">{nobody}</div>
    ) : (
      <ul className="space-y-1 text-sm">
        {access.map((entry, index) => (
          <li key={`${entry.kind}-${entry.id ?? ''}`} className="flex items-center gap-2">
            <span className="flex-1 min-w-0 truncate">
              {entry.kind === 'everyone' ? KIND_LABELS.everyone : (<>
                <span className="text-xs text-gray-500">{KIND_LABELS[entry.kind]}: </span>
                {entry.name ?? <span className="italic text-gray-500">gone</span>}
              </>)}
            </span>
            <GrantSelect
              value={entry.grant}
              grants={grants}
              onChange={(grant) => onChange(access.map((other, i) => i === index ? {...other, grant} : other))}
              label={`What ${entry.name ?? 'it'} may do`}
            />
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
    <AddMembers grants={grants} onAdd={(added) => onChange(withEntry(access, added))} />
    <AddTeam grants={grants} onAdd={(added) => onChange(withEntry(access, added))} />
  </div>
);

/** `access` with `added`, in the place of an entry naming the same members. */
function withEntry(access: AccessEntry[], added: AccessEntry): AccessEntry[] {
  return [...access.filter(e => e.kind !== added.kind || e.id !== added.id), added];
}

interface GrantSelectProps {
  value: Grant;
  grants: GrantLabels;
  onChange: (grant: Grant) => void;
  label: string;
}

const GrantSelect: React.FC<GrantSelectProps> = ({value, grants, onChange, label}) => (
  <select
    value={value}
    onChange={(e) => onChange(e.target.value as Grant)}
    className="rounded-md border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-1 py-0.5 text-xs"
    aria-label={label}
  >
    <option value="read">{grants.read}</option>
    <option value="write">{grants.write}</option>
    {(grants.join !== undefined || value === 'join') && <option value="join">{grants.join ?? 'May join'}</option>}
  </select>
);

/** The value of the second choice that names the session as a whole. */
const WHOLE_SESSION = '';

interface AddMembersProps {
  grants: GrantLabels;
  /** Adds the entry, in the place of one naming the same members. */
  onAdd: (entry: AccessEntry) => void;
}

/**
 * Picks the members of a session the user may read, or of one of its
 * channels, and what they may do - reading unless chosen otherwise.
 */
const AddMembers: React.FC<AddMembersProps> = ({grants, onAdd}) => {
  const [sessions, setSessions] = useState<SessionResponse[]>([]);
  const [sessionId, setSessionId] = useState('');
  const [channels, setChannels] = useState<ChannelResponse[]>([]);
  const [channelId, setChannelId] = useState(WHOLE_SESSION);
  const [grant, setGrant] = useState<Grant>('read');

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
    api.getChannels(sessionId)
      .then(found => { if (current) setChannels(found); })
      .catch(error => reportError('Failed to fetch the channels', error));
    return () => { current = false; };
  }, [sessionId]);

  const choose = (chosen: string) => {
    setChannels([]);
    setChannelId(WHOLE_SESSION);
    setSessionId(chosen);
  };

  const session = sessions.find(s => s.id === sessionId);
  const channel = channels.find(c => c.id === channelId);

  const add = () => {
    if (!session) return;
    onAdd(channel
      ? {kind: 'channel', id: channel.id, grant, name: channel.name}
      : {kind: 'session', id: session.id, grant, name: session.name});
    choose('');
  };

  if (sessions.length === 0) return null;

  const field = "rounded-md border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-1 py-0.5 text-xs";
  return (
    <div className="flex flex-wrap items-center gap-2 pt-1 text-xs">
      <span className="text-gray-500">Share with the members of</span>
      <select value={sessionId} onChange={(e) => choose(e.target.value)} className={field} aria-label="The session">
        <option value="">a session …</option>
        {sessions.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}
      </select>
      {session && (
        <>
          <select value={channelId} onChange={(e) => setChannelId(e.target.value)} className={field} aria-label="The whole session or one of its channels">
            <option value={WHOLE_SESSION}>the whole session</option>
            {channels.map(c => <option key={c.id} value={c.id}>the channel {c.name}</option>)}
          </select>
          <GrantSelect value={grant} grants={grants} onChange={setGrant} label="What they may do" />
          <button
            type="button"
            onClick={add}
            className="flex items-center gap-1 rounded-md border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800 px-2 py-0.5"
          >
            <Plus size={12} /> Share
          </button>
        </>
      )}
    </div>
  );
};

interface AddTeamProps {
  grants: GrantLabels;
  /** Adds the entry, in the place of one naming the same team. */
  onAdd: (entry: AccessEntry) => void;
}

/**
 * Picks a team the user may read (issue 286) - one they own or are in; the
 * server names no other - and what its members may do, reading unless
 * chosen otherwise.
 */
const AddTeam: React.FC<AddTeamProps> = ({grants, onAdd}) => {
  const [groups, setGroups] = useState<GroupResponse[]>([]);
  const [groupId, setGroupId] = useState('');
  const [grant, setGrant] = useState<Grant>('read');

  useEffect(() => {
    let current = true;
    api.getGroups()
      .then(found => { if (current) setGroups(found); })
      .catch(error => reportError('Failed to fetch the teams', error));
    return () => { current = false; };
  }, []);

  const group = groups.find(g => g.id === groupId);

  const add = () => {
    if (!group) return;
    onAdd({kind: 'group', id: group.id, grant, name: group.name});
    setGroupId('');
  };

  if (groups.length === 0) return null;

  const field = "rounded-md border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-1 py-0.5 text-xs";
  return (
    <div className="flex flex-wrap items-center gap-2 pt-1 text-xs">
      <span className="text-gray-500">Share with</span>
      <select value={groupId} onChange={(e) => setGroupId(e.target.value)} className={field} aria-label="The team">
        <option value="">a team …</option>
        {groups.map(g => <option key={g.id} value={g.id}>{g.name}</option>)}
      </select>
      {group && (
        <>
          <GrantSelect value={grant} grants={grants} onChange={setGrant} label="What its members may do" />
          <button
            type="button"
            onClick={add}
            className="flex items-center gap-1 rounded-md border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800 px-2 py-0.5"
          >
            <Plus size={12} /> Share
          </button>
        </>
      )}
    </div>
  );
};
