// Adding a participant of the session to a running channel (issue 46), with
// how much of the channel's history they may see - or, for whoever may change
// the session, someone found by name who is not in it yet, who is taken into
// the session as well (issue 221), or a new agent of a personality, made and
// then taken in (issue 227): made in the user's scope or, as they choose or
// its personality requires, in the session's or the channel's; deleted again
// if it cannot be taken in.

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import type {ActorId, Candidate, CandidateRefusal, ChannelResponse, CreateAgentRequest, HistoryAccess, PersonalityResponse, SessionParticipant} from '../types';
import {api} from '../api';
import {reportError, reportUnlessRefused} from '../errors';
import {actorKey, agentActor} from '../actor';

type HistoryKind = HistoryAccess['kind'];

const HISTORY_CHOICES: {kind: HistoryKind; label: string}[] = [
  {kind: 'all', label: 'All of it'},
  {kind: 'none', label: 'None - only what is written from now on'},
  {kind: 'last_messages', label: 'The last messages'},
  {kind: 'last_days', label: 'The last days'},
];

function historyAccess(kind: HistoryKind, count: number): HistoryAccess {
  return kind === 'last_messages' || kind === 'last_days' ? {kind, count} : {kind};
}

/** Why someone found cannot be added, as the user is told. */
const REFUSALS: Record<CandidateRefusal, string> = {
  not_cleared: 'not cleared for this channel',
  out_of_scope: 'belongs to another session or channel',
  cannot_run: 'cannot run: its personality is not its to use',
};

/** The least of a name the server searches by. */
const MIN_PREFIX = 2;

/** Whether `personality` makes an agent that may take part in `channelId` of `sessionId`. */
function admitted(personality: PersonalityResponse, sessionId: string, channelId: string): boolean {
  if (personality.scope !== 'session') return true;
  return personality.session_id === sessionId && (!personality.channel_id || personality.channel_id === channelId);
}

/**
 * Where a new agent of `personality` is made: where its personality lies if
 * that is the session or the channel, in the session if `onlyHere`, else in
 * the user's own scope.
 */
function placeOf(personality: PersonalityResponse, onlyHere: boolean, sessionId: string): Pick<CreateAgentRequest, 'scope' | 'session_id' | 'channel_id'> {
  if (personality.scope === 'session') return {scope: 'session', session_id: sessionId, channel_id: personality.channel_id};
  return onlyHere ? {scope: 'session', session_id: sessionId} : {scope: 'user'};
}

interface AddParticipantDialogProps {
  sessionId: string;
  channel: ChannelResponse;
  onAdded: (channel: ChannelResponse) => void;
  onClose: () => void;
}

export const AddParticipantDialog: React.FC<AddParticipantDialogProps> = ({sessionId, channel, onAdded, onClose}) => {
  // The participants of the session who are not in the channel yet; `null`
  // while they are loaded.
  const [candidates, setCandidates] = useState<SessionParticipant[] | null>(null);
  // Whether the user may change the session, and so search whom else to add.
  const [canSearch, setCanSearch] = useState(false);
  const [prefix, setPrefix] = useState('');
  // Those found by name who are in neither the channel nor the session's
  // list above; `null` while nothing was searched.
  const [found, setFound] = useState<Candidate[] | null>(null);
  const [chosen, setChosen] = useState<string>('');
  const [historyKind, setHistoryKind] = useState<HistoryKind>('all');
  const [count, setCount] = useState(10);
  const [problem, setProblem] = useState<string | null>(null);
  const [isAdding, setIsAdding] = useState(false);
  // A new agent instead (issue 227): the personalities it may be made of
  // here, `null` until they are asked for; its personality, name and
  // whether it is made for this session alone.
  const [makingAgent, setMakingAgent] = useState(false);
  const [personalities, setPersonalities] = useState<PersonalityResponse[] | null>(null);
  const [personalityId, setPersonalityId] = useState('');
  const [agentName, setAgentName] = useState('');
  const [onlyHere, setOnlyHere] = useState(false);

  useEffect(() => {
    const participants = new Set(channel.participants.map(m => actorKey(m.id)));
    api.getSession(sessionId)
      .then(session => {
        const outside = (session.participants ?? [])
          .filter(p => !participants.has(actorKey(p.id)))
          .sort((l, r) => l.name.localeCompare(r.name));
        setCandidates(outside);
        setCanSearch(session.can_change);
        setChosen(outside[0] ? actorKey(outside[0].id) : '');
      })
      .catch(error => reportError('Failed to load the participants of the session', error));
  }, [sessionId, channel]);

  // Searched as the user types, a moment after they stopped; what was found
  // for a shorter prefix is not shown.
  const searching = canSearch && prefix.trim().length >= MIN_PREFIX;
  useEffect(() => {
    if (!searching) return;
    let current = true;
    const timer = setTimeout(() => {
      api.searchCandidates(sessionId, prefix.trim(), channel.id)
        .then(result => { if (current) setFound(result.filter(c => !c.in_channel)); })
        .catch(error => reportError('Failed to search whom to add', error));
    }, 250);
    return () => { current = false; clearTimeout(timer); };
  }, [searching, prefix, sessionId, channel.id]);

  useEffect(() => {
    if (!makingAgent || personalities !== null) return;
    let current = true;
    api.getPersonalities()
      .then(all => {
        if (!current) return;
        const here = all.filter(p => admitted(p, sessionId, channel.id));
        setPersonalities(here);
        setPersonalityId(here[0]?.id ?? '');
      })
      .catch(error => reportError('Failed to fetch the personalities', error));
    return () => { current = false; };
  }, [makingAgent, personalities, sessionId, channel.id]);

  const shown = searching ? found : null;
  const fromSearch = shown?.find(c => actorKey(c.id) === chosen && !c.refusal);
  const participant = candidates?.find(p => actorKey(p.id) === chosen) ?? fromSearch;
  const needsCount = historyKind === 'last_messages' || historyKind === 'last_days';
  const personality = personalities?.find(p => p.id === personalityId);
  const ready = makingAgent ? !!(personality && agentName.trim()) : !!participant;

  /** Takes `id` into the channel - and the session, if it is not in it yet. */
  const takeIn = (id: ActorId) => api.addParticipant(sessionId, channel.id, {participant: id, history: historyAccess(historyKind, count)});

  /** Makes the new agent, then takes it in; one that cannot be taken in is deleted again. */
  const makeAndTakeIn = async (made: PersonalityResponse): Promise<ChannelResponse> => {
    const agent = await api.createAgent({name: agentName.trim(), personality_id: made.id, ...placeOf(made, onlyHere, sessionId)});
    try {
      return await takeIn(agentActor(agent.id));
    } catch (error) {
      await api.deleteAgent(agent.id).catch(cleanup => reportError('Failed to delete the agent that could not be added', cleanup));
      throw error;
    }
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!ready || isAdding) return;
    setIsAdding(true);
    setProblem(null);
    try {
      if (makingAgent && personality) {
        onAdded(await makeAndTakeIn(personality));
      } else if (participant) {
        onAdded(await takeIn(participant.id));
      }
    } catch (error) {
      // What the server says about the request is shown here, next to what
      // can be corrected; anything else as usual.
      reportUnlessRefused('Failed to add the participant', error, setProblem);
    } finally {
      setIsAdding(false);
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
        aria-labelledby="add-participant-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="add-participant-title" className="flex-1 font-semibold text-sm truncate">Add to {channel.name}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          {candidates === null ? (
            <div className="text-xs italic text-gray-500">Loading…</div>
          ) : (<>
            {candidates.length === 0 ? (
              <div className="text-xs italic text-gray-500">Everyone in this session takes part already.</div>
            ) : (
              <label className="block space-y-1">
                <span className="text-xs text-gray-400">From the session</span>
                <select
                  autoFocus
                  value={candidates.some(p => actorKey(p.id) === chosen) ? chosen : ''}
                  onChange={(e) => setChosen(e.target.value)}
                  className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
                >
                  {!candidates.some(p => actorKey(p.id) === chosen) && <option value="">Someone found below</option>}
                  {candidates.map(p => (
                    <option key={actorKey(p.id)} value={actorKey(p.id)}>{p.name} ({p.kind})</option>
                  ))}
                </select>
              </label>
            )}

            {canSearch && (
              <div className="space-y-1">
                <label className="block space-y-1">
                  <span className="text-xs text-gray-400">Someone not in the session yet</span>
                  <input
                    type="search"
                    value={prefix}
                    onChange={(e) => setPrefix(e.target.value)}
                    placeholder="The beginning of their name"
                    className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
                  />
                </label>
                {shown !== null && (shown.length === 0 ? (
                  <div className="text-xs italic text-gray-500">Nobody else of that name.</div>
                ) : (
                  <ul className="space-y-1">
                    {shown.map(c => (
                      <li key={actorKey(c.id)}>
                        <label className={`flex items-start gap-2 ${c.refusal ? 'text-gray-500' : ''}`}>
                          <input
                            type="radio"
                            name="found"
                            disabled={!!c.refusal}
                            checked={chosen === actorKey(c.id)}
                            onChange={() => setChosen(actorKey(c.id))}
                            className="mt-1"
                          />
                          <span>
                            {c.name} ({c.kind})
                            <span className="block text-xs text-gray-500">
                              {c.refusal ? `Cannot be added: ${REFUSALS[c.refusal]}`
                                : c.in_session ? 'In the session' : 'Not in the session yet - will be added to it'}
                            </span>
                          </span>
                        </label>
                      </li>
                    ))}
                  </ul>
                ))}
              </div>
            )}

            {canSearch && (
              <div className="space-y-2">
                <label className="flex items-center gap-2">
                  <input type="checkbox" checked={makingAgent} onChange={(e) => setMakingAgent(e.target.checked)} />
                  <span>A new agent instead</span>
                </label>
                {makingAgent && (personalities === null ? (
                  <div className="text-xs italic text-gray-500">Loading…</div>
                ) : personalities.length === 0 ? (
                  <div className="text-xs italic text-gray-500">There is no personality you may make an agent of here.</div>
                ) : (
                  <div className="space-y-2 pl-6">
                    <label className="block space-y-1">
                      <span className="text-xs text-gray-400">Name</span>
                      <input
                        value={agentName}
                        onChange={(e) => setAgentName(e.target.value)}
                        placeholder="reviewer"
                        className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
                      />
                    </label>
                    <label className="block space-y-1">
                      <span className="text-xs text-gray-400">Personality</span>
                      <select
                        value={personalityId}
                        onChange={(e) => setPersonalityId(e.target.value)}
                        className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
                      >
                        {personalities.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}
                      </select>
                      {personality?.description && <span className="block text-xs text-gray-500">{personality.description}</span>}
                    </label>
                    {personality?.scope === 'session' ? (
                      <div className="text-xs text-gray-500">
                        Its personality is of this {personality.channel_id ? 'channel' : 'session'}: the agent takes part here alone, and goes with it.
                      </div>
                    ) : (
                      <label className="flex items-start gap-2">
                        <input type="checkbox" checked={onlyHere} onChange={(e) => setOnlyHere(e.target.checked)} className="mt-1" />
                        <span>
                          For this session alone
                          <span className="block text-xs text-gray-500">
                            {onlyHere ? 'It goes with the session.' : 'It is yours, and can take part in other sessions too.'}
                          </span>
                        </span>
                      </label>
                    )}
                  </div>
                ))}
              </div>
            )}

            <fieldset className="space-y-1">
              <legend className="text-xs text-gray-400">History they may see</legend>
              {HISTORY_CHOICES.map(choice => (
                <label key={choice.kind} className="flex items-center gap-2">
                  <input
                    type="radio"
                    name="history"
                    checked={historyKind === choice.kind}
                    onChange={() => setHistoryKind(choice.kind)}
                  />
                  <span>{choice.label}</span>
                </label>
              ))}
              {needsCount && (
                <label className="flex items-center gap-2 pl-6">
                  <input
                    type="number"
                    min={1}
                    value={count}
                    onChange={(e) => setCount(Math.max(1, Math.floor(Number(e.target.value) || 1)))}
                    className="w-20 rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
                  />
                  <span className="text-xs text-gray-400">{historyKind === 'last_days' ? 'days' : 'messages'}</span>
                </label>
              )}
              <div className="text-xs text-gray-500">Notes stay visible to everyone in the channel.</div>
            </fieldset>
          </>)}

          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={!ready || isAdding}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isAdding ? 'Adding…' : 'Add'}
          </button>
        </div>
      </form>
    </div>
  );
};
