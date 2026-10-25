// The paper clip that attaches files to a message being written (issue 282): it opens the
// browser's file picker and hands over what was chosen.

import React, {useRef} from 'react';
import {Paperclip} from 'lucide-react';
import {cn} from '../classNames';

interface AttachButtonProps {
  /** Takes the files chosen. */
  onFiles: (files: File[]) => void;
  className?: string;
}

export const AttachButton: React.FC<AttachButtonProps> = ({onFiles, className}) => {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        onChange={e => {
          onFiles([...(e.target.files ?? [])]);
          // Chosen again, the same file is attached again.
          e.target.value = '';
        }}
      />
      <button
        type="button"
        onClick={() => input.current?.click()}
        className={cn("p-2 rounded-md text-gray-500 hover:text-gray-800 dark:hover:text-gray-200", className)}
        title="Attach files"
        aria-label="Attach files"
      >
        <Paperclip size={20} />
      </button>
    </>
  );
};
