// One team (issue 286), in the place of a channel: its name, where it lies and who is in it. Its
// owner renames and deletes it from its menu, takes members out and adds users met in a session
// they may read - every change writes the whole list of members. A member who does not own it
// sees who is in it and changes nothing. The participants of a session or a channel are a group
// too, read here and changed only by joining and leaving.

import React, {useEffect, useState} from 'react';
import {Edit2, Trash2, UsersRound} from 'lucide-react';
import type {GroupResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {actorKey, userIdOf} from '../actor';
import {ActionMenu} from './ActionMenu';
import {scopeLabel} from '../scope';
import {AddUser} from './AddUser';
import {EditInPlace} from './EditInPlace';

interface GroupPageProps {
  groupId: string;
  /** The user looking at it: whether it is theirs. */
  userId: string;
  onDeleted: (groupId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const GroupPage: React.FC<GroupPageProps> = ({groupId, userId, onDeleted}) => {
  const [group, setGroup] = useState<GroupResponse | null>(null);
  const [isRenaming, setIsRenaming] = useState(false);

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

  const rename = async (name: string) => {
    if (name) setGroup(await api.renameGroup(group.id, name));
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
              <EditInPlace initial={group.name} save={rename} action="Failed to rename the team" onDone={() => setIsRenaming(false)} />
            ) : (
              <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{group.name}</h2>
            )}
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
              <span>Name space: {scopeLabel(group, userId)}</span>
            </div>
          </div>
          {group.can_change && (
            <ActionMenu
              label={`Options of ${group.name}`}
              size={16}
              className="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
              actions={[
                {label: 'Rename', icon: Edit2, onSelect: () => setIsRenaming(true)},
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
            <AddUser members={memberIds} onAdd={(userId) => void setMembers([...memberIds, userId])} />
          )}
        </div>
      </div>
    </div>
  );
};
