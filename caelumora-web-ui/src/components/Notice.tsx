// A notice in the chat: what the channel, an agent or the UI tells, not something said.

import React from 'react';
import type {LucideIcon} from 'lucide-react';
import type {Message} from '../types';
import {cn} from '../classNames';
import {MessageFooter} from './MessageView';

/**
 * A notice in a dashed frame (issue 567), with an icon and a heading, what it tells below them if
 * anything, and the footer of its `message` with whatever `actions` the caller adds.
 */
export const Notice: React.FC<{
  message: Message;
  icon: LucideIcon;
  heading: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode;
}> = ({message, icon: Icon, heading, actions, children}) => (
  <div className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm">
    <div className={cn("flex items-center gap-2 text-xs font-semibold uppercase tracking-wider opacity-70", children && "mb-1")}>
      <Icon size={14} />
      {heading}
    </div>
    {children}
    <MessageFooter message={message} actions={actions} />
  </div>
);
