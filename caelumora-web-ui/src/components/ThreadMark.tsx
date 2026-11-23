// The mark of a thread at the message it hangs at (issue 595): who takes part in it, whether one
// answers there, whose sub-agent's it is - and the way into it.

import React from 'react';
import {MessagesSquare} from 'lucide-react';
import type {ChannelThread} from '../types';
import {useChatChannel} from '../chatChannel';
import {describeThread} from '../threads';
import {unreadLabel} from '../unread';

/** The mark of `thread`, which opens it in its pane. */
export const ThreadMark: React.FC<{thread: ChannelThread}> = ({thread}) => {
  const {participants, openThread} = useChatChannel();
  const {title, takingPart, answering} = describeThread(thread, participants);
  return (
    <button
      type="button"
      onClick={() => openThread(thread.id)}
      className="flex items-center gap-1.5 text-xs text-blue-600 dark:text-blue-400 hover:underline"
      title="Show this thread beside the line"
    >
      <MessagesSquare size={13} />
      <span>{title}</span>
      {takingPart.length > 0 && (
        <span className="text-gray-500 dark:text-gray-400">· {takingPart.map(m => m.name).join(', ')}</span>
      )}
      {(thread.unread?.messages ?? 0) > 0 && (
        <span className={thread.unread?.mentioned ? 'font-semibold' : 'text-gray-500 dark:text-gray-400'}>
          · {unreadLabel(thread.unread!.messages)} new{thread.unread?.mentioned ? ', mentions you' : ''}
        </span>
      )}
      {answering && (
        <span className="inline-flex items-center gap-1 text-gray-500 dark:text-gray-400">
          · answering
          <span className="w-1.5 h-1.5 bg-green-500 rounded-full animate-working" />
        </span>
      )}
    </button>
  );
};
