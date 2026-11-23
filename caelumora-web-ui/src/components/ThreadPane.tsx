// The pane of a thread beside the line (issue 595): what it is - whose sub-agent's, who takes part
// in it -, and its conversation, read and followed as the line is, written in where its circle is
// open.

import React from 'react';
import {MessagesSquare, X} from 'lucide-react';
import type {ChannelThread} from '../types';
import type {Classification} from '../confidentiality';
import type {UserInput} from '../command';
import type {Conversation} from '../conversation';
import {useChatChannel} from '../chatChannel';
import {describeThread} from '../threads';
import {ConversationView} from './ConversationView';

interface ThreadPaneProps {
  thread: ChannelThread;
  conversation: Conversation;
  /** Reads the page of its history before those shown. */
  onLoadEarlier: () => Promise<void>;
  /** Sends `input` into it, as `ConversationView` does. */
  onSend: (input: UserInput, attached: string[], release: Classification | null) => Promise<void>;
  /** Closes the pane: the channel's panel shows again. */
  onClose: () => void;
  /** What waits for the user in it: the approvals and questions of its calls. */
  children?: React.ReactNode;
}

/** The thread `thread` as its pane shows it, its conversation `conversation`. */
export const ThreadPane: React.FC<ThreadPaneProps> = ({thread, conversation, onLoadEarlier, onSend, onClose, children}) => {
  const {participants, takesPart, channel} = useChatChannel();
  const {title, takingPart} = describeThread(thread, participants);
  // A closed circle is a sub-agent's: nobody outside says anything in it (issue 602).
  const cannotWrite = !takesPart
    ? 'You do not take part in this channel: you cannot write here.'
    : thread.opening.circle === 'closed'
      ? 'Only those it was opened with speak in this thread: what it should do otherwise, tell the agent that started it.'
      : undefined;
  return (
    <div className="flex-1 flex flex-col min-h-0 min-w-0">
      <div className="flex items-center gap-2 px-4 py-2 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900">
        <MessagesSquare size={16} className="text-gray-500 dark:text-gray-400" />
        <div className="flex-1 min-w-0">
          <h3 className="text-sm font-semibold truncate">{title}</h3>
          {takingPart.length > 0 && (
            <p className="text-xs text-gray-500 dark:text-gray-400 truncate">{takingPart.map(m => m.name).join(', ')}</p>
          )}
        </div>
        <button
          type="button"
          onClick={onClose}
          className="p-1 rounded-sm text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
          title="Close the thread"
          aria-label="Close the thread"
        >
          <X size={16} />
        </button>
      </div>
      <ConversationView
        key={`${channel?.id}:${thread.id}`}
        conversation={conversation}
        onLoadEarlier={onLoadEarlier}
        draftKey={`${channel?.id}:${thread.id}`}
        onSend={onSend}
        cannotWrite={cannotWrite}
      >
        {children}
      </ConversationView>
    </div>
  );
};
