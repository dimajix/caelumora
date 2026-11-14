// A notice in the chat: what the channel, an agent or the UI tells, not something said.

import React from 'react';
import type {LucideIcon} from 'lucide-react';
import type {Message} from '../types';
import {cn} from '../classNames';
import {MessageFooter} from './MessageView';

/**
 * A notice (issue 567), with an icon and a heading, what it tells below them if anything, and the
 * footer of its `message` with whatever `actions` the caller adds: in a dashed frame (`notice`, the
 * default), or as an `alert` - what failed, announced as one (`role="alert"`) - in a solid red
 * frame, its heading not dimmed.
 */
export const Notice: React.FC<{
  message: Message;
  icon: LucideIcon;
  heading: React.ReactNode;
  tone?: 'notice' | 'alert';
  actions?: React.ReactNode;
  children?: React.ReactNode;
}> = ({message, icon: Icon, heading, tone = 'notice', actions, children}) => (
  <div
    role={tone === 'alert' ? 'alert' : undefined}
    className={cn(
      "max-w-[80%] p-3 rounded-lg border text-sm",
      tone === 'alert'
        ? "border-red-300 dark:border-red-800 bg-red-50 dark:bg-red-900/30 text-red-800 dark:text-red-200"
        : "border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200",
    )}
  >
    <div className={cn("flex items-center gap-2 text-xs font-semibold uppercase tracking-wider", tone !== 'alert' && "opacity-70", children && "mb-1")}>
      <Icon size={14} />
      {heading}
    </div>
    {children}
    <MessageFooter message={message} actions={actions} />
  </div>
);
