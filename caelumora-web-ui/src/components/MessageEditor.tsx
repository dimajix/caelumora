// Editing one's own message in place (issue 234): its text and its attachments (issue 282) - those
// it carries, each to keep or remove, and new ones attached -, saved as the message's new version.
// The message keeps its place in the chat.

import React, {useState} from 'react';
import type {ChannelAttachments} from '../api';
import type {Classification} from '../confidentiality';
import type {ContentRef} from '../types';
import {useAttachmentDraft} from '../attachmentDraft';
import {AttachButton} from './AttachButton';
import {AttachmentDraftView} from './AttachmentDraftView';
import {AttachmentList} from './AttachmentList';

interface MessageEditorProps {
  /** The text the message has now. */
  content: string;
  /** The attachments it carries now. */
  carried: ContentRef[];
  /**
   * The attachments of the channel's messages: where new ones are uploaded, classified
   * `classification` - the message's.
   */
  attachments: ChannelAttachments;
  classification?: Classification;
  /**
   * Saves `content` and the attachments `attachments`, by their ids, as the message's new version.
   */
  onSave: (content: string, attachments: string[]) => Promise<void>;
  onCancel: () => void;
}

export const MessageEditor: React.FC<MessageEditorProps> = ({content, carried, attachments, classification, onSave, onCancel}) => {
  const [text, setText] = useState(content);
  const [kept, setKept] = useState(carried);
  const draft = useAttachmentDraft(attachments);
  const [saving, setSaving] = useState(false);
  const added = draft.ids();
  const unchanged = text === content && kept.length === carried.length && draft.items.length === 0;
  const empty = !text.trim() && kept.length === 0 && draft.items.length === 0;

  const save = async () => {
    if (unchanged || empty || added === null || saving) return;
    setSaving(true);
    try {
      await onSave(text, [...kept.map(attachment => attachment.id), ...added]);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="ml-auto w-full max-w-[80%] p-3 rounded-lg rounded-br-none border border-blue-300 dark:border-blue-700 bg-white dark:bg-gray-900">
      <textarea
        value={text}
        onChange={e => setText(e.target.value)}
        onKeyDown={e => {
          if (e.key === 'Escape') onCancel();
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) void save();
        }}
        onPaste={e => draft.paste(e, classification)}
        autoFocus
        rows={Math.min(8, Math.max(2, text.split('\n').length))}
        aria-label="The message's new text"
        className="w-full p-2 text-sm border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
      />
      {kept.length > 0 && (
        <div className="text-gray-700 dark:text-gray-200">
          <AttachmentList items={kept} onRemove={id => setKept(prev => prev.filter(attachment => attachment.id !== id))} />
        </div>
      )}
      <div className="mt-2">
        <AttachmentDraftView draft={draft} />
      </div>
      <div className="flex items-center justify-end gap-2 text-xs">
        <AttachButton onFiles={files => draft.add(files, classification)} className="p-1 mr-1" />
        <span className="mr-auto opacity-60">The agents are given the new version from their next answer on.</span>
        <button type="button" onClick={onCancel} className="px-2 py-1 rounded-md border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800">
          Cancel
        </button>
        <button
          type="button"
          onClick={() => void save()}
          disabled={unchanged || empty || added === null || saving}
          className="px-2 py-1 rounded-md bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white"
        >
          Save
        </button>
      </div>
    </div>
  );
};
