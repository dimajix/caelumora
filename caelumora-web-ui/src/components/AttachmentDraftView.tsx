// The attachments of a message not yet sent (issue 282), as `useAttachmentDraft` keeps them: each
// with its name, size, a preview of an image, whether it is still uploading or why it was refused,
// and a remove.

import React from 'react';
import {File, LoaderCircle, X} from 'lucide-react';
import type {AttachmentDraft} from '../attachmentDraft';
import {labelOtherThan, type Classification} from '../confidentiality';
import {cn} from '../classNames';
import {formatSize} from '../messages';

interface AttachmentDraftViewProps {
  draft: AttachmentDraft;
  /** The channel's classification: an attachment released otherwise says how. */
  channelClassification?: Classification;
}

export const AttachmentDraftView: React.FC<AttachmentDraftViewProps> = ({draft, channelClassification}) => {
  if (draft.items.length === 0) return null;
  return (
    <ul className="flex flex-wrap gap-2 mb-2" aria-label="Attachments">
      {draft.items.map(item => {
        const failed = 'failed' in item.state ? item.state.failed : null;
        const released = item.classification && channelClassification ? labelOtherThan(item.classification, channelClassification) : null;
        return (
          <li
            key={item.key}
            title={failed ?? `${item.file.name} (${formatSize(item.file.size)})`}
            className={cn(
              "flex items-center gap-2 max-w-64 p-1 pr-2 rounded-md border text-xs",
              failed
                ? "border-red-300 dark:border-red-800 text-red-700 dark:text-red-300"
                : "border-gray-300 dark:border-gray-600 text-gray-700 dark:text-gray-200",
            )}
          >
            {item.preview
              ? <img src={item.preview} alt="" className="h-8 w-8 object-cover rounded-sm" />
              : <File size={16} className="shrink-0 opacity-70" />}
            <div className="min-w-0">
              <div className="truncate">{item.file.name}</div>
              <div className="opacity-60 truncate">
                {failed ?? [formatSize(item.file.size), released && `released: ${released}`].filter(Boolean).join(' · ')}
              </div>
            </div>
            {'uploading' in item.state && <LoaderCircle size={14} className="shrink-0 animate-spin opacity-70" aria-label="Uploading" />}
            <button
              type="button"
              onClick={() => draft.remove(item.key)}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title={`Remove ${item.file.name}`}
              aria-label={`Remove ${item.file.name}`}
            >
              <X size={13} />
            </button>
          </li>
        );
      })}
    </ul>
  );
};
