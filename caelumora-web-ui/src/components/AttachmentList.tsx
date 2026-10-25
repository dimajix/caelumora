// The attachments at a message (issue 282) - what it carries, or what a tool's result showed -:
// images as previews, linked to their full size, everything else with name, type, size and a
// download, each reached through the message that carries it; in the editor of a message, each
// with a remove.

import React, {useState} from 'react';
import {Download, File, X} from 'lucide-react';
import type {ContentRef} from '../types';
import type {ChannelAttachments} from '../api';
import {labelOtherThan, type Classification} from '../confidentiality';
import {formatSize} from '../messages';

interface AttachmentListProps {
  items: ContentRef[];
  /** The message that carries them; with `attachments`, where they are downloaded from. */
  messageId?: string;
  /** The attachments of the channel's messages; without them, or without `messageId`, they are only named. */
  attachments?: ChannelAttachments;
  /** How what carries them is classified - a message, a tool's result -: one classified otherwise says how. */
  classification?: Classification;
  /** Removes one, by its id - of a message being edited; offered where given. */
  onRemove?: (id: string) => void;
}

export const AttachmentList: React.FC<AttachmentListProps> = ({items, messageId, attachments, classification, onRemove}) => {
  if (items.length === 0) return null;
  return (
    <div className="mt-2 flex flex-wrap gap-2">
      {items.map(attachment => (
        <Attachment
          key={attachment.id}
          attachment={attachment}
          url={attachments && messageId ? attachments.url(messageId, attachment.id) : null}
          label={classification ? labelOtherThan(attachment.classification, classification) : null}
          onRemove={onRemove && (() => onRemove(attachment.id))}
        />
      ))}
    </div>
  );
};

/** One attachment: its preview where it is an image that can be fetched, else its name, type and size. */
const Attachment: React.FC<{attachment: ContentRef; url: string | null; label: string | null; onRemove?: () => void}> = ({attachment, url, label, onRemove}) => {
  // An image that cannot be shown - not found, not readable, one the server sends only as a
  // download - is named as any other file.
  const [broken, setBroken] = useState(false);
  const what = `${attachment.name} (${attachment.media_type}, ${formatSize(attachment.size)})`;
  if (url && !broken && attachment.media_type.startsWith('image/')) {
    return (
      <a href={url} target="_blank" rel="noreferrer" title={what} className="block">
        <img src={url} alt={attachment.name} onError={() => setBroken(true)} className="max-h-48 max-w-full rounded-sm border border-black/10" />
        {label && <span className="text-[11px] opacity-70">{label}</span>}
      </a>
    );
  }
  return (
    <div className="flex items-center gap-2 px-2 py-1 rounded-sm border border-current/20 text-xs max-w-full" title={what}>
      <File size={14} className="shrink-0 opacity-70" />
      <span className="truncate">{attachment.name}</span>
      <span className="opacity-60 whitespace-nowrap">{attachment.media_type} · {formatSize(attachment.size)}</span>
      {label && <span className="opacity-70 whitespace-nowrap">{label}</span>}
      {url && (
        <a href={url} download={attachment.name} className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100" title={`Download ${attachment.name}`} aria-label={`Download ${attachment.name}`}>
          <Download size={13} />
        </a>
      )}
      {onRemove && (
        <button type="button" onClick={onRemove} className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100" title={`Remove ${attachment.name}`} aria-label={`Remove ${attachment.name}`}>
          <X size={13} />
        </button>
      )}
    </div>
  );
};
