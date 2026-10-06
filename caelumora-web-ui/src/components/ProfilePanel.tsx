// Who a participant is (issue 244): how it is introduced - an agent by its
// personality, a user in their own words - and of an agent the name of its
// personality, whether it runs here or elsewhere, and who owns it. All of
// it is public to whoever reads the channel; read once when the panel opens.

import React, {useEffect, useState} from 'react';
import {IdCard, X} from 'lucide-react';
import type {ChannelMember} from '../types';
import {api} from '../api';
import {actorUuid} from '../mention';
import {reportError} from '../errors';

interface ProfilePanelProps {
  sessionId: string;
  channelId: string;
  /** The participant whose profile is shown. */
  member: ChannelMember;
  onClose: () => void;
}

export const ProfilePanel: React.FC<ProfilePanelProps> = ({sessionId, channelId, member, onClose}) => {
  const actor = actorUuid(member.id);
  const [profile, setProfile] = useState<ChannelMember | null>(null);

  useEffect(() => {
    if (!actor) return;
    let current = true;
    api.getParticipant(sessionId, channelId, actor)
      .then(response => { if (current) setProfile(response); })
      .catch(error => reportError('Failed to load the profile', error));
    return () => { current = false; };
  }, [sessionId, channelId, actor]);

  const isAgent = member.kind === 'agent';

  return (
    <div className="w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex items-center gap-2 text-gray-700 dark:text-gray-200">
          <IdCard size={18} />
          <h3 className="font-semibold text-sm flex-1">About {member.name}</h3>
          <button onClick={onClose} className="p-1 hover:bg-gray-100 dark:hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="text-xs text-gray-500 dark:text-gray-400">
          {isAgent
            ? 'How the agent is introduced to everybody in the channel - the other agents read the same.'
            : 'How they introduce themselves to everybody in the channel - the agents read the same.'}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-4 text-sm">
        {profile === null ? (
          <div className="text-gray-500 dark:text-gray-400">Loading…</div>
        ) : (
          <>
            <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
              <dt className="text-gray-500 dark:text-gray-400">Name</dt>
              <dd className="text-gray-800 dark:text-gray-100">{profile.name}</dd>
              {profile.handle && (<>
                <dt className="text-gray-500 dark:text-gray-400">Handle</dt>
                <dd className="text-gray-800 dark:text-gray-100">@{profile.handle}</dd>
              </>)}
              {isAgent && (<>
                <dt className="text-gray-500 dark:text-gray-400">Personality</dt>
                <dd className="text-gray-800 dark:text-gray-100">{profile.personality ?? 'unknown'}</dd>
                <dt className="text-gray-500 dark:text-gray-400">Runs</dt>
                <dd
                  className="text-gray-800 dark:text-gray-100"
                  title={profile.personality_kind === 'external' ? 'What it is given leaves this system' : undefined}
                >
                  {profile.personality_kind === 'external' ? 'elsewhere - what it is given leaves this system' : profile.personality_kind === 'internal' ? 'here' : 'unknown'}
                </dd>
                <dt className="text-gray-500 dark:text-gray-400">Owner</dt>
                <dd className="text-gray-800 dark:text-gray-100" title="Whose authority it acts with, and who answers for its tool calls">
                  {profile.owner_name ?? 'unknown'}
                </dd>
              </>)}
            </dl>

            <div className="space-y-1">
              <div className="text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">
                {isAgent ? 'Introduction' : 'In their own words'}
              </div>
              {profile.introduction ? (
                <div className="whitespace-pre-wrap text-gray-800 dark:text-gray-100">{profile.introduction}</div>
              ) : (
                <div className="text-gray-500 dark:text-gray-400">
                  {isAgent ? 'Its personality does not introduce it.' : 'They have not introduced themselves.'}
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
};
