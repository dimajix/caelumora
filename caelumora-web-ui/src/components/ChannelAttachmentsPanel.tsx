// What was attached in a channel (issue 509): its messages carrying attachments, the newest first,
// each with who said it and when - as far as the user's view of the history reaches, a page at a
// time. What a tool's result gave its model is no attachment and is not listed.

import React, {useEffect, useState} from 'react';
import type {HistoryPage, Message} from '../types';
import type {ChannelAttachments} from '../api';
import {reportError} from '../errors';
import {AttachmentList} from './AttachmentList';
import {MessageTime} from './MessageTime';

interface ChannelAttachmentsPanelProps {
  /** The attachments of the channel's messages: where they are listed and downloaded from. */
  attachments: ChannelAttachments;
  /**
   * Changes whenever what the channel's messages carry may have changed - a message with
   * attachments said, a message changed, taken back or reclassified, the channel reclassified, the
   * stream started from a snapshot -: the list is read again from its end.
   */
  version: number;
}

/** The messages of `page`, the newest first. */
const carrying = (page: HistoryPage<Message>): Message[] =>
  page.items.flatMap(item => 'said' in item ? [item.said] : []).reverse();

export const ChannelAttachmentsPanel: React.FC<ChannelAttachmentsPanelProps> = ({attachments, version}) => {
  // The messages read, and what the page before them is read with - `null` once there is none;
  // `undefined` while the first page is read.
  const [read, setRead] = useState<{messages: Message[]; earlier: number | null} | undefined>(undefined);
  const [isLoading, setIsLoading] = useState(false);

  useEffect(() => {
    let current = true;
    attachments.list()
      .then(page => { if (current) setRead({messages: carrying(page), earlier: page.earlier}); })
      .catch(error => {
        if (!current) return;
        reportError('Failed to load the attachments', error);
        setRead({messages: [], earlier: null});
      });
    return () => { current = false; };
  }, [attachments, version]);

  /**
   * Puts the page before those shown after them - unless the list was read again from its end
   * meanwhile, which the page would no longer follow.
   */
  const loadEarlier = async () => {
    const before = read?.earlier;
    if (before == null) return;
    setIsLoading(true);
    try {
      const page = await attachments.list(before);
      setRead(prev => prev && prev.earlier === before
        ? {messages: [...prev.messages, ...carrying(page)], earlier: page.earlier}
        : prev);
    } catch (error) {
      reportError('Failed to load the attachments', error);
    } finally {
      setIsLoading(false);
    }
  };

  if (read === undefined) return null;
  if (read.messages.length === 0) {
    return <div className="text-xs italic text-gray-500">Nothing was attached here.</div>;
  }
  return (
    <div className="space-y-4">
      {read.messages.map(message => (
        <div key={message.id}>
          <div className="flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
            <span className="truncate font-medium text-gray-700 dark:text-gray-300">{message.sender_name || 'Someone'}</span>
            <MessageTime timestamp={message.timestamp} />
          </div>
          <AttachmentList
            items={message.attachments ?? []}
            messageId={message.id}
            attachments={attachments}
            classification={message.classification}
          />
        </div>
      ))}
      {read.earlier !== null && (
        <div className="text-center">
          <button
            onClick={loadEarlier}
            disabled={isLoading}
            className="text-xs px-3 py-1 rounded-md border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800 disabled:opacity-50"
          >
            {isLoading ? 'Loading…' : 'Load earlier attachments'}
          </button>
        </div>
      )}
    </div>
  );
};
