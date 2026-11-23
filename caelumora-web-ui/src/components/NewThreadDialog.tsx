// Opening a thread at a message of the line (issue 595): the user's own agents to found it with
// beside them - one new to the channel takes part in the thread alone, to work apart with. Whether
// it may be opened, and with whom, is the server's to decide.

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import type {ActorId, AgentResponse, ChannelThread, Message} from '../types';
import {api} from '../api';
import {reportError, reportUnlessRefused} from '../errors';
import {sameActor, userActor} from '../actor';
import {Refusal} from './Refusal';

interface NewThreadDialogProps {
  sessionId: string;
  channelId: string;
  /** The message of the line it hangs at. */
  message: Message;
  /** Who opens it: their own agents are offered. */
  userId: string;
  onOpened: (thread: ChannelThread) => void;
  onClose: () => void;
}

export const NewThreadDialog: React.FC<NewThreadDialogProps> = ({sessionId, channelId, message, userId, onOpened, onClose}) => {
  // The user's own agents, once fetched.
  const [agents, setAgents] = useState<AgentResponse[] | null>(null);
  const [chosen, setChosen] = useState<string[]>([]);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [isOpening, setIsOpening] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const me = userActor(userId);
    api.getAgents()
      .then(found => { if (!cancelled) setAgents(found.filter(agent => sameActor(agent.owner, me))); })
      .catch(error => reportError('Failed to load your agents', error));
    return () => { cancelled = true; };
  }, [userId]);

  const toggle = (id: string) =>
    setChosen(prev => prev.includes(id) ? prev.filter(c => c !== id) : [...prev, id]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (isOpening || !message.id) return;
    setIsOpening(true);
    setRefusal(null);
    try {
      const founders: ActorId[] = chosen.map(id => ({Agent: id}));
      onOpened(await api.openThread(sessionId, channelId, message.id, founders));
      onClose();
    } catch (error) {
      reportUnlessRefused('Failed to open the thread', error, setRefusal);
    } finally {
      setIsOpening(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
      onKeyDown={(e) => { if (e.key === 'Escape') onClose(); }}
    >
      <form
        onSubmit={submit}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-thread-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="new-thread-title" className="flex-1 font-semibold text-sm truncate">Reply in a thread</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-3 p-4 text-sm">
          <p className="text-xs text-gray-400 line-clamp-3">{message.content}</p>
          <p className="text-xs text-gray-400">
            Bring agents of yours into it - one that takes no part in the channel yet takes part in this thread alone.
          </p>
          {agents === null ? (
            <p className="text-xs text-gray-500">Loading your agents…</p>
          ) : agents.length === 0 ? (
            <p className="text-xs text-gray-500">You have no agents of your own.</p>
          ) : (
            <ul className="max-h-48 overflow-y-auto space-y-1">
              {agents.map(agent => (
                <li key={agent.id}>
                  <label className="flex items-center gap-2">
                    <input type="checkbox" checked={chosen.includes(agent.id)} onChange={() => toggle(agent.id)} />
                    <span>{agent.name}</span>
                    {agent.personality && <span className="text-xs text-gray-500">{agent.personality}</span>}
                  </label>
                </li>
              ))}
            </ul>
          )}
          <Refusal message={refusal} />
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={isOpening}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isOpening ? 'Opening…' : 'Start'}
          </button>
        </div>
      </form>
    </div>
  );
};
