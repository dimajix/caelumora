// What an agent can do now (issue 194): its tools in this channel, by the
// tool set they come from, as it would offer them at the start of its next
// turn - read from the running agent, since an agent's tools are built for
// each of its channels. A set that is unavailable says why. Each tool shows
// what only the runtime acts on: whether a call needs approval, who sees
// it, how its results are classified.

import React, {useEffect, useState} from 'react';
import {Wrench, X} from 'lucide-react';
import type {ChannelMember, ToolSet} from '../types';
import {api} from '../api';
import {actorUuid} from '../mention';
import {ApiRequestError, reportError} from '../errors';
import {ToolSetList} from './ToolSetList';

interface ToolsPanelProps {
  sessionId: string;
  channelId: string;
  /** The agent whose tools are shown. */
  member: ChannelMember;
  onClose: () => void;
}

export const ToolsPanel: React.FC<ToolsPanelProps> = ({sessionId, channelId, member, onClose}) => {
  const actor = actorUuid(member.id);
  const [sets, setSets] = useState<ToolSet[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!actor) return;
    let current = true;
    api.getAgentTools(sessionId, channelId, actor)
      .then(response => { if (current) setSets(response.tool_sets); })
      .catch(error => {
        if (error instanceof ApiRequestError && error.code === 'agent_forbidden') {
          if (current) setNotice('You may not see which tools this agent has.');
        } else {
          reportError('Failed to load the tools', error);
        }
      });
    return () => { current = false; };
  }, [sessionId, channelId, actor]);

  const count = sets?.reduce((total, set) => total + set.tools.length, 0) ?? 0;

  return (
    <div className="w-[28rem] max-w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex items-center gap-2 text-gray-700 dark:text-gray-200">
          <Wrench size={18} />
          <h3 className="font-semibold text-sm flex-1">The tools of {member.name}</h3>
          <button onClick={onClose} className="p-1 hover:bg-gray-100 dark:hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="text-xs text-gray-500 dark:text-gray-400">
          What the agent can call in this channel now, by where the tools come from, as it would offer them at the
          start of its next turn.
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {notice && <div className="text-sm text-gray-500 dark:text-gray-400">{notice}</div>}
        {sets === null && !notice && <div className="text-sm text-gray-500 dark:text-gray-400">Loading…</div>}
        {sets?.length === 0 && (
          <div className="text-sm text-gray-500 dark:text-gray-400">It has no tools here.</div>
        )}
        {sets && sets.length > 0 && (
          <>
            <div className="text-xs text-gray-500 dark:text-gray-400">{count} {count === 1 ? 'tool' : 'tools'} in {sets.length} {sets.length === 1 ? 'set' : 'sets'}</div>
            <ToolSetList sets={sets} />
          </>
        )}
      </div>
    </div>
  );
};
