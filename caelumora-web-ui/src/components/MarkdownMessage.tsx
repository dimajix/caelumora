import React, {useMemo} from 'react';
import ReactMarkdown, {type Components} from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkBreaks from 'remark-breaks';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';
import {mentionSpans} from '../mention';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

const MENTION_URL_PREFIX = '#mention:';

// Just enough of the mdast shape for the mention plugin below.
interface MdNode {
  type: string;
  value?: string;
  url?: string;
  children?: MdNode[];
}

/**
 * Remark plugin turning resolved `@`-mentions in text nodes into links to
 * `#mention:<handle>`, which the `a` renderer below displays as a mention.
 * Code and existing links are left alone; unresolved `@…` stays plain text.
 */
function remarkMentions(handles: Set<string>) {
  const split = (node: MdNode): MdNode[] => {
    const text = node.value ?? '';
    const result: MdNode[] = [];
    let pos = 0;
    for (const span of mentionSpans(text)) {
      if (!handles.has(span.handle)) continue;
      if (span.start > pos) result.push({type: 'text', value: text.slice(pos, span.start)});
      result.push({
        type: 'link',
        url: MENTION_URL_PREFIX + span.handle,
        children: [{type: 'text', value: text.slice(span.start, span.end)}],
      });
      pos = span.end;
    }
    if (pos === 0) return [node];
    if (pos < text.length) result.push({type: 'text', value: text.slice(pos)});
    return result;
  };

  const walk = (node: MdNode) => {
    if (!node.children || node.type === 'link' || node.type === 'linkReference') return;
    node.children = node.children.flatMap(child => {
      if (child.type === 'text') return split(child);
      walk(child);
      return [child];
    });
  };

  return () => (tree: MdNode) => {
    if (handles.size > 0) walk(tree);
  };
}

interface MarkdownMessageProps {
  content: string;
  className?: string;
  /** Force light-on-dark ("inverted") prose styling, e.g. for messages on a solid colored background. */
  invert?: boolean;
  /** Handles of the participants this message mentions; matching `@handle`s are highlighted. */
  mentions?: Set<string>;
  /**
   * Show every line break as one, not only the Markdown ones (a blank line, two trailing spaces):
   * for text a user typed into a chat field, where a line break is meant as written (issue 274).
   */
  hardBreaks?: boolean;
}

export const MarkdownMessage: React.FC<MarkdownMessageProps> = ({ content, className, invert, mentions, hardBreaks }) => {
  const remarkPlugins = useMemo(
    () => [remarkGfm, ...(hardBreaks ? [remarkBreaks] : []), remarkMentions(mentions ?? new Set())],
    [mentions, hardBreaks]
  );
  const components = useMemo<Components>(() => ({
    a: ({href, title, children}) => {
      if (href?.startsWith(MENTION_URL_PREFIX)) {
        return (
          <span
            className={cn(
              'rounded-sm px-1 font-semibold no-underline',
              invert
                ? 'bg-white/20 text-white'
                : 'bg-blue-100 text-blue-800 dark:bg-blue-900/50 dark:text-blue-200'
            )}
          >
            {children}
          </span>
        );
      }
      return <a href={href} title={title}>{children}</a>;
    },
  }), [invert]);

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
        'prose-a:wrap-break-word wrap-break-word',
        className
      )}
    >
      <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>{content}</ReactMarkdown>
    </div>
  );
};
