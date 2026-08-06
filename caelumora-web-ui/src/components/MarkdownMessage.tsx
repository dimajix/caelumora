import React from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

interface MarkdownMessageProps {
  content: string;
  className?: string;
  /** Force light-on-dark ("inverted") prose styling, e.g. for messages on a solid colored background. */
  invert?: boolean;
}

export const MarkdownMessage: React.FC<MarkdownMessageProps> = ({ content, className, invert }) => {
  return (
    <div
      className={cn(
        'prose prose-sm max-w-none',
        invert ? 'prose-invert' : 'dark:prose-invert',
        'prose-p:my-1.5 prose-pre:my-2 prose-ul:my-1.5 prose-ol:my-1.5 prose-headings:my-2',
        invert
          ? 'prose-pre:bg-black/20 prose-pre:border prose-pre:border-white/20'
          : 'prose-pre:bg-gray-100 dark:prose-pre:bg-gray-900 prose-pre:border prose-pre:border-gray-200 dark:prose-pre:border-gray-700',
        'prose-code:before:content-none prose-code:after:content-none',
        'prose-a:break-words break-words',
        className
      )}
    >
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
    </div>
  );
};
