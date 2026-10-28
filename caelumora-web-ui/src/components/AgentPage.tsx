// One agent (issue 227), in the place of a channel, on its own rather than as a participant of one:
// its name, where it lies, its personality and whose it is, and the channels it takes part in
// that the user may read, each opened from here. Whoever may renames and deletes it from its
// menu; deleted, it rests in its channels until it is restored.

import React, {useEffect, useState} from 'react';
import {Bot, Edit2, MessageSquare, Trash2} from 'lucide-react';
import type {AgentResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {ActionMenu} from './ActionMenu';
import {scopeLabel} from '../scope';
import {userIdOf} from '../actor';
import {EditInPlace} from './EditInPlace';

interface AgentPageProps {
  agentId: string;
  /** The user looking at it: whether it is theirs. */
  userId: string;
  onDeleted: (agentId: string) => void;
  /** Opens a channel it takes part in. */
  onSelectChannel: (sessionId: string, channelId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const AgentPage: React.FC<AgentPageProps> = ({agentId, userId, onDeleted, onSelectChannel}) => {
  const [agent, setAgent] = useState<AgentResponse | null>(null);
  const [isRenaming, setIsRenaming] = useState(false);

  useEffect(() => {
    let current = true;
    api.getAgent(agentId)
      .then(found => { if (current) setAgent(found); })
      .catch(error => reportError('Failed to load the agent', error));
    return () => { current = false; };
  }, [agentId]);

  if (!agent) return null;

  const rename = async (name: string) => {
    if (!name) return;
    // Answered without its channels, which a rename does not change.
    const renamed = await api.renameAgent(agent.id, name);
    setAgent({...renamed, channels: agent.channels});
  };

  const remove = async () => {
    try {
      await api.deleteAgent(agent.id);
      onDeleted(agent.id);
    } catch (error) {
      reportError('Failed to delete the agent', error);
    }
  };

  const owner = userIdOf(agent.owner) === userId ? 'you' : 'someone else';
  const actions = [
    ...(agent.can_change ? [{label: 'Rename', icon: Edit2, onSelect: () => setIsRenaming(true)}] : []),
    ...(agent.can_delete ? [{label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete agent?', onSelect: () => void remove()}] : []),
  ];
  const channels = agent.channels ?? [];

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <div className="flex items-start gap-3">
          <Bot size={22} className="mt-1 shrink-0 text-gray-400" />
          <div className="flex-1 min-w-0 space-y-1">
            {isRenaming ? (
              <EditInPlace initial={agent.name} save={rename} action="Failed to rename the agent" onDone={() => setIsRenaming(false)} />
            ) : (
              <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{agent.name}</h2>
            )}
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
              <span>Personality: {agent.personality ?? <span className="italic">gone</span>}</span>
              <span>Name space: {scopeLabel(agent, userId)}</span>
              <span>Owner: {owner}</span>
            </div>
          </div>
          {actions.length > 0 && (
            <ActionMenu
              label={`Options of ${agent.name}`}
              size={16}
              className="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
              actions={actions}
            />
          )}
        </div>

        <div className="text-sm text-gray-600 dark:text-gray-400">
          {agent.scope === 'session'
            ? `It takes part only in ${agent.channel_id ? 'one channel' : 'one session'}, and goes with it.`
            : 'It acts with the rights of its owner, in every channel it takes part in.'}
        </div>

        <div className="space-y-1">
          <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Channels</h3>
          {channels.length === 0 ? (
            <div className="text-xs italic text-gray-500">It takes part in no channel you may read.</div>
          ) : (
            <ul className="space-y-1 text-sm">
              {channels.map(channel => (
                <li key={channel.channel_id}>
                  <button
                    type="button"
                    onClick={() => onSelectChannel(channel.session_id, channel.channel_id)}
                    className="flex items-center gap-2 rounded-md px-1 -mx-1 hover:bg-gray-100 dark:hover:bg-gray-800 text-left"
                  >
                    <MessageSquare size={14} className="shrink-0 text-gray-400" />
                    <span className="truncate">{channel.session_name} / {channel.channel_name}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
};
