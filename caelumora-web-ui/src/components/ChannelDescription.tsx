// What the channel is for (issue 245), at the top of the channel panel: its
// summary, and its briefing behind a click, rendered as Markdown. Whoever may
// change the channel is offered to change both, and to write them where the
// channel says nothing yet.

import React, {useState} from 'react';
import {ChevronDown, ChevronRight, FileText, Pencil} from 'lucide-react';
import type {ChannelResponse} from '../types';
import {MarkdownMessage} from './MarkdownMessage';

interface ChannelDescriptionProps {
  channel: ChannelResponse;
  onEdit: () => void;
}

export const ChannelDescription: React.FC<ChannelDescriptionProps> = ({channel, onEdit}) => {
  const [showBriefing, setShowBriefing] = useState(false);

  if (!channel.summary && !channel.briefing) {
    return channel.can_change ? (
      <button
        onClick={onEdit}
        className="flex items-center gap-2 mb-4 text-xs text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
      >
        <FileText size={14} />
        <span>Say what this channel is for</span>
      </button>
    ) : null;
  }

  return (
    <div className="mb-4 space-y-1 text-sm">
      <div className="flex items-start gap-2">
        <span className="flex-1 text-gray-800 dark:text-gray-100">{channel.summary}</span>
        {channel.can_change && (
          <button
            onClick={onEdit}
            className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100 text-gray-500 dark:text-gray-400"
            title="Change what the channel is for"
            aria-label="Change the description of the channel"
          >
            <Pencil size={12} />
          </button>
        )}
      </div>
      {channel.briefing && (
        <>
          <button
            onClick={() => setShowBriefing(shown => !shown)}
            className="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
            aria-expanded={showBriefing}
            title="What its agents are told before they work here"
          >
            {showBriefing ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            Briefing
          </button>
          {showBriefing && (
            <div className="rounded-md border border-gray-200 dark:border-gray-700 p-2 text-xs">
              <MarkdownMessage content={channel.briefing} />
            </div>
          )}
        </>
      )}
    </div>
  );
};
