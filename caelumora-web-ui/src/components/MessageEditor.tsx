// Editing one's own message in place (issue 234): its text, saved as the
// message's new version. The message keeps its place in the chat.

import React, {useState} from 'react';

interface MessageEditorProps {
  /** The text the message has now. */
  content: string;
  /** Saves `content` as the message's new version. */
  onSave: (content: string) => Promise<void>;
  onCancel: () => void;
}

export const MessageEditor: React.FC<MessageEditorProps> = ({content, onSave, onCancel}) => {
  const [text, setText] = useState(content);
  const [saving, setSaving] = useState(false);
  const unchanged = !text.trim() || text === content;

  const save = async () => {
    if (unchanged || saving) return;
    setSaving(true);
    try {
      await onSave(text);
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
        autoFocus
        rows={Math.min(8, Math.max(2, text.split('\n').length))}
        aria-label="The message's new text"
        className="w-full p-2 text-sm border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
      />
      <div className="mt-2 flex items-center justify-end gap-2 text-xs">
        <span className="mr-auto opacity-60">The agents are given the new text from their next answer on.</span>
        <button type="button" onClick={onCancel} className="px-2 py-1 rounded-md border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800">
          Cancel
        </button>
        <button
          type="button"
          onClick={() => void save()}
          disabled={unchanged || saving}
          className="px-2 py-1 rounded-md bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white"
        >
          Save
        </button>
      </div>
    </div>
  );
};
